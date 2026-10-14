//! The mock backend and its sessions.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use rvisa_core::{
    Attr, AttrValue, Backend, Error, Event, EventKind, FlowControl, InterfaceType, LockKind,
    OpenOptions, Parity, Pattern, ReadResult, ResourceName, Result, Session, StopBits, TermReason,
};

use crate::bytes::escape;
use crate::script::{Action, Script, demo_script};

const BACKEND: &str = "mock";
/// How often blocked operations re-check shared state.
const POLL: Duration = Duration::from_millis(1);
/// The status byte's request-service bit.
const RQS: u8 = 0x40;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// A response waiting to be read.
#[derive(Debug)]
struct Pending {
    data: Vec<u8>,
    ready_at: Instant,
}

#[derive(Debug, Default)]
struct SessionEvents {
    enabled: HashSet<EventKind>,
    queue: VecDeque<EventKind>,
}

#[derive(Debug, Default)]
struct Locks {
    /// The session holding an exclusive lock, and its nesting depth.
    exclusive: Option<(u64, u32)>,
    /// The shared lock's key and each holder's nesting depth.
    shared: Option<(String, HashMap<u64, u32>)>,
}

impl Locks {
    /// Whether `session` is locked out of I/O.
    fn blocks(&self, session: u64) -> bool {
        let exclusive = self.exclusive.is_some_and(|(owner, _)| owner != session);
        let shared = self
            .shared
            .as_ref()
            .is_some_and(|(_, holders)| !holders.contains_key(&session));
        exclusive || shared
    }

    fn try_lock(&mut self, session: u64, kind: &LockKind) -> bool {
        match kind {
            LockKind::Exclusive => match &mut self.exclusive {
                Some((owner, depth)) if *owner == session => {
                    *depth += 1;
                    true
                }
                Some(_) => false,
                None => {
                    let others_share = self
                        .shared
                        .as_ref()
                        .is_some_and(|(_, h)| h.keys().any(|&s| s != session));
                    if others_share {
                        return false;
                    }
                    self.exclusive = Some((session, 1));
                    true
                }
            },
            LockKind::Shared { key } => {
                if self.exclusive.is_some_and(|(owner, _)| owner != session) {
                    return false;
                }
                match &mut self.shared {
                    Some((held, holders)) if held == key => {
                        *holders.entry(session).or_insert(0) += 1;
                        true
                    }
                    Some(_) => false,
                    None => {
                        self.shared = Some((key.clone(), HashMap::from([(session, 1)])));
                        true
                    }
                }
            }
        }
    }

    fn unlock(&mut self, session: u64) -> bool {
        if let Some((owner, depth)) = &mut self.exclusive
            && *owner == session
        {
            *depth -= 1;
            if *depth == 0 {
                self.exclusive = None;
            }
            return true;
        }
        if let Some((_, holders)) = &mut self.shared
            && let Some(depth) = holders.get_mut(&session)
        {
            *depth -= 1;
            if *depth == 0 {
                holders.remove(&session);
            }
            if holders.is_empty() {
                self.shared = None;
            }
            return true;
        }
        false
    }

    fn release_all(&mut self, session: u64) {
        while self.unlock(session) {}
    }
}

/// The state of one simulated instrument, shared by its sessions.
#[derive(Debug)]
struct State {
    resource: ResourceName,
    script: Script,
    output: VecDeque<Pending>,
    writes: Vec<Vec<u8>>,
    stb: u8,
    triggers: u32,
    clears: u32,
    locks: Locks,
    sessions: HashMap<u64, SessionEvents>,
}

impl State {
    fn new(resource: ResourceName, script: Script) -> Self {
        Self {
            resource,
            script,
            output: VecDeque::new(),
            writes: Vec::new(),
            stb: 0,
            triggers: 0,
            clears: 0,
            locks: Locks::default(),
            sessions: HashMap::new(),
        }
    }

    fn raise(&mut self, kind: EventKind) {
        for events in self.sessions.values_mut() {
            if events.enabled.contains(&kind) {
                events.queue.push_back(kind);
            }
        }
    }

    fn handle_write(&mut self, data: &[u8]) -> Result<()> {
        self.writes.push(data.to_vec());
        let Some(rule) = self.script.find(data) else {
            if self.script.strict {
                return Err(Error::Protocol {
                    backend: BACKEND,
                    detail: format!("unexpected write {}", escape(data)),
                });
            }
            return Ok(());
        };
        let mut ready_at = Instant::now();
        for action in rule.actions.clone() {
            match action {
                Action::Delay(d) => ready_at += d,
                Action::Respond(response) => self.output.push_back(Pending {
                    data: response.to_vec(),
                    ready_at,
                }),
                Action::Srq(stb) => {
                    self.stb = stb | RQS;
                    self.raise(EventKind::ServiceRequest);
                }
                Action::Timeout => {
                    tracing::debug!(resource = %self.resource, "mock: injected timeout");
                }
            }
        }
        Ok(())
    }
}

type Shared = Arc<Mutex<State>>;

fn lock(state: &Shared) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A scripted mock backend.
///
/// Each registered instrument keeps its state (output queue, status byte,
/// locks) across all sessions opened on it, as a real device does.
///
/// ```
/// use rvisa_core::{Backend, OpenOptions, SessionExt};
/// use rvisa_mock::{MockBackend, Script};
///
/// let rn = "TCPIP0::dmm.local::INSTR".parse()?;
/// let backend = MockBackend::new().instrument(
///     rn,
///     Script::new().on_write("*IDN?\n").respond(b"ACME,DMM,1,1.0\n"),
/// );
/// let mut session = backend.open(&"TCPIP::dmm.local".parse()?, &OpenOptions::new())?;
/// assert_eq!(session.query_bytes(b"*IDN?\n")?, b"ACME,DMM,1,1.0\n");
/// # Ok::<(), rvisa_core::Error>(())
/// ```
#[derive(Debug, Default)]
pub struct MockBackend {
    instruments: Mutex<HashMap<ResourceName, Shared>>,
    next_session: AtomicU64,
}

impl MockBackend {
    /// A backend with no instruments.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A backend preloaded with demonstration instruments, all running
    /// [`demo_script`]:
    ///
    /// - `TCPIP0::mock.local::INSTR`
    /// - `TCPIP0::mock.local::5025::SOCKET`
    /// - `USB0::0x1234::0x5678::MOCK001::INSTR`
    /// - `GPIB0::22::INSTR`
    /// - `ASRL1::INSTR`
    #[must_use]
    pub fn demo() -> Self {
        let backend = Self::new();
        for (resource, model) in DEMO_INSTRUMENTS {
            if let Ok(rn) = resource.parse() {
                backend.add_instrument(rn, demo_script(model, "MOCK001"));
            }
        }
        backend
    }

    /// Adds an instrument (builder style). Replaces any existing instrument
    /// with the same name.
    #[must_use]
    pub fn instrument(self, resource: ResourceName, script: Script) -> Self {
        self.add_instrument(resource, script);
        self
    }

    /// Adds an instrument. Replaces any existing instrument with the same
    /// name; its open sessions keep the old state.
    pub fn add_instrument(&self, resource: ResourceName, script: Script) {
        let state = Arc::new(Mutex::new(State::new(resource.clone(), script)));
        self.instruments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(resource, state);
    }

    /// A handle for inspecting and driving an instrument from a test.
    #[must_use]
    pub fn handle(&self, resource: &ResourceName) -> Option<MockInstrument> {
        self.get(resource).map(|state| MockInstrument { state })
    }

    fn get(&self, resource: &ResourceName) -> Option<Shared> {
        self.instruments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(resource)
            .cloned()
    }
}

const DEMO_INSTRUMENTS: [(&str, &str); 5] = [
    ("TCPIP0::mock.local::INSTR", "DMM1000"),
    ("TCPIP0::mock.local::5025::SOCKET", "DMM1000"),
    ("USB0::0x1234::0x5678::MOCK001::INSTR", "SCOPE200"),
    ("GPIB0::22::INSTR", "PSU30"),
    ("ASRL1::INSTR", "LOAD10"),
];

impl Backend for MockBackend {
    fn name(&self) -> &'static str {
        BACKEND
    }

    fn supports(&self, resource: &ResourceName) -> bool {
        self.get(resource).is_some()
    }

    fn open(&self, resource: &ResourceName, options: &OpenOptions) -> Result<Box<dyn Session>> {
        let state = self
            .get(resource)
            .ok_or_else(|| Error::NotFound(resource.clone()))?;
        let id = self.next_session.fetch_add(1, Ordering::Relaxed);
        lock(&state).sessions.insert(id, SessionEvents::default());
        let mut session = MockSession::new(resource.clone(), state, id);
        if let Some(kind) = &options.lock {
            session.lock(kind.clone(), options.open_timeout)?;
        }
        tracing::debug!(%resource, "mock: opened session");
        Ok(Box::new(session))
    }

    fn list(&self, pattern: &str) -> Result<Vec<ResourceName>> {
        let pattern = Pattern::new(pattern)?;
        let mut names: Vec<_> = self
            .instruments
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .filter(|rn| pattern.matches(rn))
            .cloned()
            .collect();
        names.sort_by_key(ToString::to_string);
        Ok(names)
    }
}

/// A handle to a mock instrument, for tests to inspect and drive it.
#[derive(Debug, Clone)]
pub struct MockInstrument {
    state: Shared,
}

impl MockInstrument {
    /// Every write received, in order.
    #[must_use]
    pub fn writes(&self) -> Vec<Vec<u8>> {
        lock(&self.state).writes.clone()
    }

    /// The number of triggers received.
    #[must_use]
    pub fn triggers(&self) -> u32 {
        lock(&self.state).triggers
    }

    /// The number of device clears received.
    #[must_use]
    pub fn clears(&self) -> u32 {
        lock(&self.state).clears
    }

    /// The current status byte.
    #[must_use]
    pub fn stb(&self) -> u8 {
        lock(&self.state).stb
    }

    /// Queues a response message, readable at once.
    pub fn push_response(&self, data: impl AsRef<[u8]>) {
        lock(&self.state).output.push_back(Pending {
            data: data.as_ref().to_vec(),
            ready_at: Instant::now(),
        });
    }

    /// Sets the status byte (with RQS) and raises a service request.
    pub fn raise_srq(&self, stb: u8) {
        let mut state = lock(&self.state);
        state.stb = stb | RQS;
        state.raise(EventKind::ServiceRequest);
    }
}

/// A session on a mock instrument.
#[derive(Debug)]
pub struct MockSession {
    resource: ResourceName,
    state: Shared,
    id: u64,
    attrs: HashMap<Attr, AttrValue>,
}

impl MockSession {
    fn new(resource: ResourceName, state: Shared, id: u64) -> Self {
        let (manufacturer, model) = {
            let s = lock(&state);
            (
                s.script
                    .manufacturer
                    .clone()
                    .unwrap_or_else(|| "Mock".into()),
                s.script.model.clone().unwrap_or_else(|| "Mock".into()),
            )
        };
        let mut attrs = HashMap::from([
            (
                Attr::TimeoutValue,
                AttrValue::Timeout(Some(DEFAULT_TIMEOUT)),
            ),
            (Attr::TermChar, AttrValue::U8(b'\n')),
            (Attr::TermCharEnabled, AttrValue::Bool(false)),
            (Attr::SendEndEnabled, AttrValue::Bool(true)),
            (Attr::SuppressEndEnabled, AttrValue::Bool(false)),
            (Attr::ManufacturerName, AttrValue::Str(manufacturer)),
            (Attr::ModelName, AttrValue::Str(model)),
            (Attr::ResourceName, AttrValue::Str(resource.to_string())),
        ]);
        let interface = match &resource {
            ResourceName::TcpipInstr { .. } | ResourceName::TcpipSocket { .. } => {
                attrs.insert(Attr::TcpipKeepalive, AttrValue::Bool(false));
                attrs.insert(Attr::TcpipNodelay, AttrValue::Bool(true));
                InterfaceType::Tcpip
            }
            ResourceName::UsbInstr { serial, .. } => {
                attrs.insert(Attr::UsbSerialNumber, AttrValue::Str(serial.clone()));
                InterfaceType::Usb
            }
            ResourceName::GpibInstr { .. } => InterfaceType::Gpib,
            _ => {
                attrs.insert(Attr::AsrlBaud, AttrValue::U32(9600));
                attrs.insert(Attr::AsrlDataBits, AttrValue::U16(8));
                attrs.insert(Attr::AsrlParity, AttrValue::Parity(Parity::None));
                attrs.insert(Attr::AsrlStopBits, AttrValue::StopBits(StopBits::One));
                attrs.insert(
                    Attr::AsrlFlowControl,
                    AttrValue::FlowControl(FlowControl::NONE),
                );
                InterfaceType::Asrl
            }
        };
        attrs.insert(Attr::InterfaceType, AttrValue::InterfaceType(interface));
        Self {
            resource,
            state,
            id,
            attrs,
        }
    }

    fn term_char(&self) -> Option<u8> {
        let enabled = self
            .attrs
            .get(&Attr::TermCharEnabled)
            .and_then(AttrValue::as_bool)
            .unwrap_or(false);
        match self.attrs.get(&Attr::TermChar) {
            Some(AttrValue::U8(c)) if enabled => Some(*c),
            _ => None,
        }
    }

    /// The state, or `ResourceLocked` if another session's lock blocks I/O.
    fn access(&self) -> Result<MutexGuard<'_, State>> {
        let state = lock(&self.state);
        if state.locks.blocks(self.id) {
            return Err(Error::ResourceLocked);
        }
        Ok(state)
    }

    fn unsupported(op: &'static str) -> Error {
        Error::Unsupported {
            backend: BACKEND,
            op,
        }
    }
}

/// Sleeps until `deadline`, or returns `false` at once if it is `None`.
fn sleep_until(deadline: Option<Instant>) -> bool {
    match deadline {
        Some(deadline) => {
            let now = Instant::now();
            if deadline > now {
                thread::sleep((deadline - now).min(POLL));
            }
            true
        }
        None => false,
    }
}

impl Session for MockSession {
    fn resource(&self) -> &ResourceName {
        &self.resource
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        tracing::trace!(resource = %self.resource, data = %escape(data), "mock: write");
        self.access()?.handle_write(data)?;
        Ok(data.len())
    }

    fn read(&mut self, max: usize) -> Result<ReadResult> {
        if max == 0 {
            return Err(Error::InvalidArgument("read of zero bytes".into()));
        }
        let term = self.term_char();
        let deadline = self.timeout().map(|t| Instant::now() + t);
        loop {
            let mut state = self.access()?;
            let now = Instant::now();
            let ready = match state.output.front() {
                Some(front) if front.ready_at <= now => true,
                // A delayed response that arrives before the deadline.
                Some(front) if deadline.is_none_or(|d| front.ready_at <= d) => false,
                // Nothing will arrive in time.
                _ => {
                    drop(state);
                    // With an infinite timeout and nothing coming, fail
                    // rather than hang (DESIGN.md M1).
                    match deadline {
                        Some(d) if Instant::now() < d => {
                            sleep_until(Some(d));
                            continue;
                        }
                        _ => return Err(Error::Timeout),
                    }
                }
            };
            if !ready {
                let wait = state.output.front().map(|f| f.ready_at);
                drop(state);
                sleep_until(wait);
                continue;
            }
            let Some(front) = state.output.front_mut() else {
                continue;
            };
            let mut n = front.data.len().min(max);
            if let Some(c) = term
                && let Some(pos) = front.data[..n].iter().position(|&b| b == c)
            {
                n = pos + 1;
            }
            let data = front.data[..n].to_vec();
            front.data.drain(..n);
            let reason = if front.data.is_empty() {
                state.output.pop_front();
                TermReason::End
            } else if term.is_some() && data.last() == term.as_ref() {
                TermReason::TermChar
            } else {
                TermReason::MaxCount
            };
            tracing::trace!(resource = %self.resource, data = %escape(&data), ?reason, "mock: read");
            return Ok(ReadResult { data, reason });
        }
    }

    fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<()> {
        self.attrs
            .insert(Attr::TimeoutValue, AttrValue::Timeout(timeout));
        Ok(())
    }

    fn timeout(&self) -> Option<Duration> {
        self.attrs
            .get(&Attr::TimeoutValue)
            .and_then(AttrValue::as_timeout)
            .unwrap_or(Some(DEFAULT_TIMEOUT))
    }

    fn set_read_termination(&mut self, term: Option<u8>) -> Result<()> {
        if let Some(c) = term {
            self.attrs.insert(Attr::TermChar, AttrValue::U8(c));
        }
        self.attrs
            .insert(Attr::TermCharEnabled, AttrValue::Bool(term.is_some()));
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        let mut state = self.access()?;
        state.output.clear();
        state.clears += 1;
        Ok(())
    }

    fn read_stb(&mut self) -> Result<u8> {
        let mut state = self.access()?;
        let stb = state.stb;
        // A serial poll clears the request-service bit.
        state.stb &= !RQS;
        Ok(stb)
    }

    fn assert_trigger(&mut self) -> Result<()> {
        self.access()?.triggers += 1;
        Ok(())
    }

    fn lock(&mut self, kind: LockKind, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if lock(&self.state).locks.try_lock(self.id, &kind) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::ResourceLocked);
            }
            sleep_until(Some(deadline));
        }
    }

    fn unlock(&mut self) -> Result<()> {
        if lock(&self.state).locks.unlock(self.id) {
            Ok(())
        } else {
            Err(Error::InvalidArgument("the session holds no lock".into()))
        }
    }

    fn get_attr(&self, attr: Attr) -> Result<AttrValue> {
        self.attrs
            .get(&attr)
            .cloned()
            .ok_or_else(|| Self::unsupported("get_attr"))
    }

    fn set_attr(&mut self, attr: Attr, value: AttrValue) -> Result<()> {
        if attr.is_read_only() {
            return Err(Error::AttrReadOnly(attr));
        }
        if !self.attrs.contains_key(&attr) {
            return Err(Self::unsupported("set_attr"));
        }
        if !value.fits(attr) {
            return Err(Error::InvalidArgument(format!(
                "{attr:?} takes {:?}, not {:?}",
                attr.kind(),
                value.kind()
            )));
        }
        self.attrs.insert(attr, value);
        Ok(())
    }

    fn enable_event(&mut self, kind: EventKind) -> Result<()> {
        if kind != EventKind::ServiceRequest {
            return Err(Self::unsupported("enable_event"));
        }
        let mut state = lock(&self.state);
        state
            .sessions
            .entry(self.id)
            .or_default()
            .enabled
            .insert(kind);
        Ok(())
    }

    fn wait_event(&mut self, kind: EventKind, timeout: Duration) -> Result<Event> {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let mut state = lock(&self.state);
                let events = state.sessions.entry(self.id).or_default();
                if !events.enabled.contains(&kind) {
                    return Err(Error::EventNotEnabled(kind));
                }
                if let Some(pos) = events.queue.iter().position(|&k| k == kind) {
                    events.queue.remove(pos);
                    return Ok(Event::new(kind));
                }
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            sleep_until(Some(deadline));
        }
    }

    fn disable_event(&mut self, kind: EventKind) -> Result<()> {
        let mut state = lock(&self.state);
        let events = state.sessions.entry(self.id).or_default();
        events.enabled.remove(&kind);
        events.queue.retain(|&k| k != kind);
        Ok(())
    }
}

impl Drop for MockSession {
    fn drop(&mut self) {
        let mut state = lock(&self.state);
        state.locks.release_all(self.id);
        state.sessions.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvisa_core::SessionExt;

    fn rn(s: &str) -> ResourceName {
        s.parse().unwrap()
    }

    fn demo_session(backend: &MockBackend, name: &str) -> Box<dyn Session> {
        backend.open(&rn(name), &OpenOptions::new()).unwrap()
    }

    #[test]
    fn list_and_supports() {
        let backend = MockBackend::demo();
        assert_eq!(backend.name(), "mock");
        assert_eq!(backend.list("?*").unwrap().len(), 5);
        assert_eq!(
            backend.list("TCPIP?*").unwrap(),
            [
                rn("TCPIP0::mock.local::5025::SOCKET"),
                rn("TCPIP0::mock.local::INSTR")
            ]
        );
        assert!(backend.list("[").is_err());
        assert!(backend.supports(&rn("GPIB0::22")));
        assert!(!backend.supports(&rn("GPIB0::23")));
        assert!(matches!(
            backend.open(&rn("GPIB0::23"), &OpenOptions::new()),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn strict_scripts_reject_unknown_writes() {
        let name = rn("ASRL2::INSTR");
        let backend = MockBackend::new().instrument(name.clone(), Script::new().strict());
        let mut s = backend.open(&name, &OpenOptions::new()).unwrap();
        assert!(matches!(s.write(b"X\n"), Err(Error::Protocol { .. })));
        assert_eq!(backend.handle(&name).unwrap().writes(), [b"X\n".to_vec()]);
    }

    #[test]
    fn delayed_and_hung_responses() {
        let backend = MockBackend::demo();
        let mut s = demo_session(&backend, "GPIB0::22::INSTR");

        s.set_timeout(Some(Duration::from_millis(50))).unwrap();
        s.write(b"SLOW?\n").unwrap();
        let start = Instant::now();
        assert!(matches!(s.read(64), Err(Error::Timeout)));
        assert!(start.elapsed() >= Duration::from_millis(50));

        s.set_timeout(Some(Duration::from_secs(2))).unwrap();
        assert_eq!(s.read_message().unwrap(), b"done\n");

        s.write(b"HANG?\n").unwrap();
        s.set_timeout(Some(Duration::from_millis(20))).unwrap();
        assert!(matches!(s.read(64), Err(Error::Timeout)));

        s.set_timeout(None).unwrap();
        assert!(matches!(s.read(64), Err(Error::Timeout)), "never hangs");
        assert!(matches!(s.read(0), Err(Error::InvalidArgument(_))));
    }

    #[test]
    fn locks_block_other_sessions() {
        let backend = MockBackend::demo();
        let name = "TCPIP0::mock.local::INSTR";
        let mut a = demo_session(&backend, name);
        let mut b = demo_session(&backend, name);
        let short = Duration::from_millis(10);

        a.lock(LockKind::Exclusive, short).unwrap();
        a.lock(LockKind::Exclusive, short).unwrap();
        assert!(matches!(b.write(b"*IDN?\n"), Err(Error::ResourceLocked)));
        assert!(matches!(
            b.lock(LockKind::Exclusive, short),
            Err(Error::ResourceLocked)
        ));
        a.unlock().unwrap();
        assert!(
            matches!(b.read_stb(), Err(Error::ResourceLocked)),
            "still nested"
        );
        a.unlock().unwrap();
        assert!(a.unlock().is_err());
        b.write(b"*CLS\n").unwrap();

        let key = LockKind::Shared { key: "k".into() };
        a.lock(key.clone(), short).unwrap();
        assert!(matches!(b.clear(), Err(Error::ResourceLocked)));
        assert!(matches!(
            b.lock(
                LockKind::Shared {
                    key: "other".into()
                },
                short
            ),
            Err(Error::ResourceLocked)
        ));
        assert!(matches!(
            b.lock(LockKind::Exclusive, short),
            Err(Error::ResourceLocked)
        ));
        b.lock(key, short).unwrap();
        b.assert_trigger().unwrap();
        assert!(matches!(
            a.lock(LockKind::Exclusive, short),
            Err(Error::ResourceLocked)
        ));

        // Dropping a session releases its locks.
        drop(a);
        drop(b);
        let mut c = demo_session(&backend, name);
        c.lock(LockKind::Exclusive, short).unwrap();
    }

    #[test]
    fn open_with_lock() {
        let backend = MockBackend::demo();
        let name = rn("ASRL1::INSTR");
        let options = OpenOptions::new()
            .lock(LockKind::Exclusive)
            .open_timeout(Duration::from_millis(10));
        let _held = backend.open(&name, &options).unwrap();
        assert!(matches!(
            backend.open(&name, &options),
            Err(Error::ResourceLocked)
        ));
    }

    #[test]
    fn attributes() {
        let backend = MockBackend::demo();
        let mut usb = demo_session(&backend, "USB0::0x1234::0x5678::MOCK001::INSTR");
        assert_eq!(
            usb.get_attr(Attr::UsbSerialNumber).unwrap(),
            AttrValue::Str("MOCK001".into())
        );
        assert_eq!(
            usb.get_attr(Attr::InterfaceType).unwrap(),
            AttrValue::InterfaceType(InterfaceType::Usb)
        );
        assert_eq!(
            usb.get_attr(Attr::ModelName).unwrap(),
            AttrValue::Str("SCOPE200".into())
        );
        assert!(matches!(
            usb.get_attr(Attr::AsrlBaud),
            Err(Error::Unsupported { .. })
        ));
        assert!(matches!(
            usb.set_attr(Attr::AsrlBaud, AttrValue::U32(1)),
            Err(Error::Unsupported { .. })
        ));
        assert!(matches!(
            usb.set_attr(Attr::ModelName, AttrValue::Str("x".into())),
            Err(Error::AttrReadOnly(Attr::ModelName))
        ));
        assert!(matches!(
            usb.set_attr(Attr::TermChar, AttrValue::Bool(true)),
            Err(Error::InvalidArgument(_))
        ));

        let mut serial = demo_session(&backend, "ASRL1::INSTR");
        serial
            .set_attr(Attr::AsrlParity, AttrValue::Parity(Parity::Even))
            .unwrap();
        assert_eq!(
            serial.get_attr(Attr::AsrlParity).unwrap(),
            AttrValue::Parity(Parity::Even)
        );
        assert_eq!(
            serial.get_attr(Attr::InterfaceType).unwrap(),
            AttrValue::InterfaceType(InterfaceType::Asrl)
        );

        let gpib = demo_session(&backend, "GPIB0::22::INSTR");
        assert_eq!(
            gpib.get_attr(Attr::InterfaceType).unwrap(),
            AttrValue::InterfaceType(InterfaceType::Gpib)
        );
        let tcpip = demo_session(&backend, "TCPIP0::mock.local::INSTR");
        assert_eq!(
            tcpip.get_attr(Attr::TcpipNodelay).unwrap(),
            AttrValue::Bool(true)
        );
    }

    #[test]
    fn events_and_handle() {
        let backend = MockBackend::demo();
        let name = rn("GPIB0::22::INSTR");
        let mut s = backend.open(&name, &OpenOptions::new()).unwrap();
        let handle = backend.handle(&name).unwrap();
        let short = Duration::from_millis(10);

        assert!(matches!(
            s.wait_event(EventKind::ServiceRequest, short),
            Err(Error::EventNotEnabled(_))
        ));
        assert!(matches!(
            s.enable_event(EventKind::Trigger),
            Err(Error::Unsupported { .. })
        ));
        s.enable_event(EventKind::ServiceRequest).unwrap();
        assert!(matches!(
            s.wait_event(EventKind::ServiceRequest, short),
            Err(Error::Timeout)
        ));
        handle.raise_srq(0x01);
        assert_eq!(handle.stb(), 0x41);
        assert_eq!(
            s.wait_event(EventKind::ServiceRequest, short).unwrap().kind,
            EventKind::ServiceRequest
        );
        assert_eq!(s.read_stb().unwrap(), 0x41);
        assert_eq!(s.read_stb().unwrap(), 0x01);

        handle.raise_srq(0);
        s.disable_event(EventKind::ServiceRequest).unwrap();
        s.enable_event(EventKind::ServiceRequest).unwrap();
        assert!(
            matches!(
                s.wait_event(EventKind::ServiceRequest, short),
                Err(Error::Timeout)
            ),
            "disabling discards queued events"
        );

        handle.push_response(b"pushed\n");
        assert_eq!(s.read_message().unwrap(), b"pushed\n");
        s.assert_trigger().unwrap();
        s.clear().unwrap();
        assert_eq!((handle.triggers(), handle.clears()), (1, 1));
    }
}
