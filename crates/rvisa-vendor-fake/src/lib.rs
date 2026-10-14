//! A fake VISA shared library for testing `rvisa-vendor` without VISA
//! (DESIGN.md V2). Test-only; never published.
//!
//! It exports the VISA C functions `rvisa-vendor` uses and translates each
//! call into a call on an `rvisa-mock` session of [`MockBackend::demo`], so
//! the vendor backend runs through real `dlopen`, real C calling
//! conventions and real pointer marshalling.
//!
//! The alias `mockdmm` resolves to `TCPIP0::mock.local::INSTR`.

// The exports are the VISA C API: their contract is VPP-4.3, and they must
// dereference caller-provided pointers.
#![allow(unsafe_code, missing_docs, clippy::missing_safety_doc)]

use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, c_char, c_void};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use rvisa_core::{
    Attr, AttrValue, Backend, Error, EventKind, FlowControl, InterfaceType, LockKind, OpenOptions,
    Parity, ResourceName, Session, StopBits, TermReason,
};
use rvisa_mock::MockBackend;
use rvisa_vendor::visa::*;

type Shared = Arc<Mutex<Box<dyn Session>>>;

#[derive(Default)]
struct Fake {
    next: u32,
    rms: Vec<ViSession>,
    /// Sessions and the resource manager that opened them.
    sessions: HashMap<ViSession, (ViSession, Shared)>,
    finds: HashMap<ViFindList, VecDeque<String>>,
    events: Vec<ViEvent>,
}

impl Fake {
    fn id(&mut self) -> u32 {
        self.next += 1;
        self.next
    }
}

fn backend() -> &'static MockBackend {
    static BACKEND: OnceLock<MockBackend> = OnceLock::new();
    BACKEND.get_or_init(MockBackend::demo)
}

fn state() -> MutexGuard<'static, Fake> {
    static STATE: OnceLock<Mutex<Fake>> = OnceLock::new();
    STATE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// The session for `vi`, released from the global lock before use so that
/// blocking calls on one session do not stall the others.
fn session(vi: ViSession) -> Option<Shared> {
    state().sessions.get(&vi).map(|(_, s)| Arc::clone(s))
}

fn with_session(vi: ViSession, f: impl FnOnce(&mut dyn Session) -> ViStatus) -> ViStatus {
    let Some(shared) = session(vi) else {
        return VI_ERROR_INV_OBJECT;
    };
    let mut guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
    f(guard.as_mut())
}

/// What an rvisa error looks like as a VISA status.
fn status(e: &Error) -> ViStatus {
    match e {
        Error::Timeout { .. } => VI_ERROR_TMO,
        Error::NotFound(_) => VI_ERROR_RSRC_NFOUND,
        Error::ResourceLocked => VI_ERROR_RSRC_LOCKED,
        Error::Unsupported { .. } => VI_ERROR_NSUP_OPER,
        Error::AttrReadOnly(_) => VI_ERROR_ATTR_READONLY,
        Error::EventNotEnabled(_) => VI_ERROR_NENABLED,
        Error::InvalidResource(_) => VI_ERROR_INV_RSRC_NAME,
        Error::InvalidArgument(_) => VI_ERROR_INV_PARAMETER,
        _ => VI_ERROR_SYSTEM_ERROR,
    }
}

fn result(r: rvisa_core::Result<()>) -> ViStatus {
    r.map_or_else(|e| status(&e), |()| VI_SUCCESS)
}

unsafe fn c_str<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller passes a NUL-terminated string, per VPP-4.3.
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

/// Writes `s` into a caller buffer of at least `VI_FIND_BUFLEN` bytes.
unsafe fn write_str(dst: *mut ViChar, s: &str) {
    if dst.is_null() {
        return;
    }
    let len = s.len().min(VI_FIND_BUFLEN - 1);
    // SAFETY: VPP-4.3 requires the buffer to hold VI_FIND_BUFLEN bytes, and
    // at most VI_FIND_BUFLEN - 1 bytes plus a NUL are written.
    unsafe {
        std::ptr::copy_nonoverlapping(s.as_ptr().cast::<ViChar>(), dst, len);
        *dst.add(len) = 0;
    }
}

unsafe fn set<T>(p: *mut T, value: T) {
    if !p.is_null() {
        // SAFETY: a non-null out-pointer from the caller is valid for writes.
        unsafe { p.write(value) };
    }
}

fn timeout(ms: ViUInt32) -> Option<Duration> {
    (ms != VI_TMO_INFINITE).then(|| Duration::from_millis(ms.into()))
}

fn event_kind(event: ViEventType) -> Option<EventKind> {
    Some(match event {
        VI_EVENT_SERVICE_REQ => EventKind::ServiceRequest,
        VI_EVENT_TRIG => EventKind::Trigger,
        VI_EVENT_CLEAR => EventKind::Clear,
        VI_EVENT_EXCEPTION => EventKind::Exception,
        VI_EVENT_USB_INTR => EventKind::UsbInterrupt,
        _ => return None,
    })
}

/// The attribute and its C width in bytes (0 for strings).
fn attr(id: ViAttr) -> Option<(Attr, usize)> {
    Some(match id {
        VI_ATTR_TMO_VALUE => (Attr::TimeoutValue, 4),
        VI_ATTR_TERMCHAR => (Attr::TermChar, 1),
        VI_ATTR_TERMCHAR_EN => (Attr::TermCharEnabled, 2),
        VI_ATTR_SEND_END_EN => (Attr::SendEndEnabled, 2),
        VI_ATTR_SUPPRESS_END_EN => (Attr::SuppressEndEnabled, 2),
        VI_ATTR_ASRL_BAUD => (Attr::AsrlBaud, 4),
        VI_ATTR_ASRL_DATA_BITS => (Attr::AsrlDataBits, 2),
        VI_ATTR_ASRL_PARITY => (Attr::AsrlParity, 2),
        VI_ATTR_ASRL_STOP_BITS => (Attr::AsrlStopBits, 2),
        VI_ATTR_ASRL_FLOW_CNTRL => (Attr::AsrlFlowControl, 2),
        VI_ATTR_TCPIP_KEEPALIVE => (Attr::TcpipKeepalive, 2),
        VI_ATTR_TCPIP_NODELAY => (Attr::TcpipNodelay, 2),
        VI_ATTR_INTF_TYPE => (Attr::InterfaceType, 2),
        VI_ATTR_MANF_NAME => (Attr::ManufacturerName, 0),
        VI_ATTR_MODEL_NAME => (Attr::ModelName, 0),
        VI_ATTR_USB_SERIAL_NUM => (Attr::UsbSerialNumber, 0),
        VI_ATTR_RSRC_NAME => (Attr::ResourceName, 0),
        _ => return None,
    })
}

fn intf_number(intf: InterfaceType) -> u16 {
    match intf {
        InterfaceType::Gpib => VI_INTF_GPIB,
        InterfaceType::Vxi => VI_INTF_VXI,
        InterfaceType::GpibVxi => VI_INTF_GPIB_VXI,
        InterfaceType::Asrl => VI_INTF_ASRL,
        InterfaceType::Pxi => VI_INTF_PXI,
        InterfaceType::Tcpip => VI_INTF_TCPIP,
        _ => VI_INTF_USB,
    }
}

/// The value as the number VISA would store.
fn to_number(value: &AttrValue) -> u64 {
    match value {
        AttrValue::Bool(b) => u64::from(*b),
        AttrValue::U8(v) => (*v).into(),
        AttrValue::U16(v) => (*v).into(),
        AttrValue::U32(v) => (*v).into(),
        AttrValue::U64(v) => *v,
        AttrValue::Timeout(t) => t.map_or(VI_TMO_INFINITE.into(), |d| {
            u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
        }),
        AttrValue::Parity(p) => u64::from(match p {
            Parity::Odd => VI_ASRL_PAR_ODD,
            Parity::Even => VI_ASRL_PAR_EVEN,
            Parity::Mark => VI_ASRL_PAR_MARK,
            Parity::Space => VI_ASRL_PAR_SPACE,
            _ => VI_ASRL_PAR_NONE,
        }),
        AttrValue::StopBits(s) => u64::from(match s {
            StopBits::OnePointFive => VI_ASRL_STOP_ONE5,
            StopBits::Two => VI_ASRL_STOP_TWO,
            _ => VI_ASRL_STOP_ONE,
        }),
        AttrValue::FlowControl(f) => f.bits().into(),
        AttrValue::InterfaceType(i) => intf_number(*i).into(),
        _ => 0,
    }
}

/// The number VISA was given, as the attribute's value.
fn from_number(attr: Attr, raw: u64) -> Option<AttrValue> {
    let u16v = u16::try_from(raw).ok();
    Some(match attr {
        Attr::TimeoutValue => AttrValue::Timeout(timeout(u32::try_from(raw).ok()?)),
        Attr::TermChar => AttrValue::U8(u8::try_from(raw).ok()?),
        Attr::TermCharEnabled
        | Attr::SendEndEnabled
        | Attr::SuppressEndEnabled
        | Attr::TcpipKeepalive
        | Attr::TcpipNodelay => AttrValue::Bool(raw != 0),
        Attr::AsrlBaud => AttrValue::U32(u32::try_from(raw).ok()?),
        Attr::AsrlDataBits => AttrValue::U16(u16v?),
        Attr::AsrlParity => AttrValue::Parity(match u16v? {
            VI_ASRL_PAR_NONE => Parity::None,
            VI_ASRL_PAR_ODD => Parity::Odd,
            VI_ASRL_PAR_EVEN => Parity::Even,
            VI_ASRL_PAR_MARK => Parity::Mark,
            VI_ASRL_PAR_SPACE => Parity::Space,
            _ => return None,
        }),
        Attr::AsrlStopBits => AttrValue::StopBits(match u16v? {
            VI_ASRL_STOP_ONE => StopBits::One,
            VI_ASRL_STOP_ONE5 => StopBits::OnePointFive,
            VI_ASRL_STOP_TWO => StopBits::Two,
            _ => return None,
        }),
        Attr::AsrlFlowControl => AttrValue::FlowControl(FlowControl::from_bits_truncate(u16v?)),
        _ => return None,
    })
}

// ---- Resource manager ----

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viOpenDefaultRM(vi: *mut ViSession) -> ViStatus {
    if vi.is_null() {
        return VI_ERROR_INV_PARAMETER;
    }
    let mut fake = state();
    let id = fake.id();
    fake.rms.push(id);
    // SAFETY: checked non-null above; the caller passes a valid out-pointer.
    unsafe { vi.write(id) };
    VI_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viOpen(
    rm: ViSession,
    name: *const ViChar,
    _mode: ViAccessMode,
    _timeout: ViUInt32,
    vi: *mut ViSession,
) -> ViStatus {
    if !state().rms.contains(&rm) {
        return VI_ERROR_INV_OBJECT;
    }
    // SAFETY: `name` is a NUL-terminated string from the caller.
    let Some(name) = (unsafe { c_str(name) }) else {
        return VI_ERROR_INV_RSRC_NAME;
    };
    let Ok(rn) = name.parse::<ResourceName>() else {
        return VI_ERROR_INV_RSRC_NAME;
    };
    match backend().open(&rn, &OpenOptions::new()) {
        Ok(session) => {
            let mut fake = state();
            let id = fake.id();
            fake.sessions
                .insert(id, (rm, Arc::new(Mutex::new(session))));
            // SAFETY: the caller passes a valid out-pointer.
            unsafe { set(vi, id) };
            VI_SUCCESS
        }
        Err(e) => status(&e),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viClose(vi: ViObject) -> ViStatus {
    let mut fake = state();
    if let Some(pos) = fake.rms.iter().position(|&r| r == vi) {
        fake.rms.remove(pos);
        // Closing a resource manager closes its sessions.
        fake.sessions.retain(|_, (rm, _)| *rm != vi);
        return VI_SUCCESS;
    }
    if fake.sessions.remove(&vi).is_some() || fake.finds.remove(&vi).is_some() {
        return VI_SUCCESS;
    }
    if let Some(pos) = fake.events.iter().position(|&e| e == vi) {
        fake.events.remove(pos);
        return VI_SUCCESS;
    }
    VI_ERROR_INV_OBJECT
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viFindRsrc(
    rm: ViSession,
    expr: *const ViChar,
    list: *mut ViFindList,
    count: *mut ViUInt32,
    desc: *mut ViChar,
) -> ViStatus {
    if !state().rms.contains(&rm) {
        return VI_ERROR_INV_OBJECT;
    }
    // SAFETY: `expr` is a NUL-terminated string from the caller.
    let Some(expr) = (unsafe { c_str(expr) }) else {
        return VI_ERROR_INV_EXPR;
    };
    let Ok(names) = backend().list(expr) else {
        return VI_ERROR_INV_EXPR;
    };
    let mut names: VecDeque<String> = names.iter().map(ToString::to_string).collect();
    let Some(first) = names.pop_front() else {
        return VI_ERROR_RSRC_NFOUND;
    };
    let mut fake = state();
    let id = fake.id();
    let total = u32::try_from(names.len() + 1).unwrap_or(u32::MAX);
    fake.finds.insert(id, names);
    // SAFETY: the out-pointers and the VI_FIND_BUFLEN buffer are valid.
    unsafe {
        set(list, id);
        set(count, total);
        write_str(desc, &first);
    }
    VI_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viFindNext(list: ViFindList, desc: *mut ViChar) -> ViStatus {
    let mut fake = state();
    let Some(names) = fake.finds.get_mut(&list) else {
        return VI_ERROR_INV_OBJECT;
    };
    let Some(next) = names.pop_front() else {
        return VI_ERROR_RSRC_NFOUND;
    };
    // SAFETY: `desc` is a VI_FIND_BUFLEN buffer from the caller.
    unsafe { write_str(desc, &next) };
    VI_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viParseRsrcEx(
    rm: ViSession,
    name: *const ViChar,
    intf_type: *mut ViUInt16,
    intf_num: *mut ViUInt16,
    class: *mut ViChar,
    expanded: *mut ViChar,
    alias: *mut ViChar,
) -> ViStatus {
    if !state().rms.contains(&rm) {
        return VI_ERROR_INV_OBJECT;
    }
    // SAFETY: `name` is a NUL-terminated string from the caller.
    let Some(name) = (unsafe { c_str(name) }) else {
        return VI_ERROR_INV_RSRC_NAME;
    };
    let (rn, used_alias) = if name.eq_ignore_ascii_case("mockdmm") {
        match "TCPIP0::mock.local::INSTR".parse::<ResourceName>() {
            Ok(rn) => (rn, name),
            Err(_) => return VI_ERROR_SYSTEM_ERROR,
        }
    } else {
        match name.parse::<ResourceName>() {
            Ok(rn) => (rn, ""),
            Err(_) => return VI_ERROR_INV_RSRC_NAME,
        }
    };
    if !backend().supports(&rn) {
        return VI_ERROR_RSRC_NFOUND;
    }
    let text = rn.to_string();
    let intf = match &rn {
        ResourceName::TcpipInstr { .. } | ResourceName::TcpipSocket { .. } => VI_INTF_TCPIP,
        ResourceName::UsbInstr { .. } => VI_INTF_USB,
        ResourceName::GpibInstr { .. } => VI_INTF_GPIB,
        _ => VI_INTF_ASRL,
    };
    let class_name = text.rsplit("::").next().unwrap_or("INSTR");
    // SAFETY: the out-pointers and VI_FIND_BUFLEN buffers are valid.
    unsafe {
        set(intf_type, intf);
        set(intf_num, rn.board().unwrap_or(0));
        write_str(class, class_name);
        write_str(expanded, &text);
        write_str(alias, used_alias);
    }
    VI_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viStatusDesc(
    _vi: ViObject,
    code: ViStatus,
    desc: *mut ViChar,
) -> ViStatus {
    let text = status_name(code).map_or_else(
        || format!("Unknown status {code:#x}"),
        |n| format!("{n} (fake)"),
    );
    // SAFETY: `desc` is a VI_FIND_BUFLEN buffer from the caller.
    unsafe { write_str(desc, &text) };
    VI_SUCCESS
}

// ---- I/O ----

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viWrite(
    vi: ViSession,
    buf: *const u8,
    count: ViUInt32,
    ret: *mut ViUInt32,
) -> ViStatus {
    if buf.is_null() && count != 0 {
        return VI_ERROR_INV_PARAMETER;
    }
    let data: &[u8] = if count == 0 {
        &[]
    } else {
        // SAFETY: the caller passes `count` readable bytes at `buf`.
        unsafe { std::slice::from_raw_parts(buf, count as usize) }
    };
    with_session(vi, |s| match s.write(data) {
        Ok(n) => {
            // SAFETY: the caller passes a valid out-pointer.
            unsafe { set(ret, u32::try_from(n).unwrap_or(u32::MAX)) };
            VI_SUCCESS
        }
        Err(e) => status(&e),
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viRead(
    vi: ViSession,
    buf: *mut u8,
    count: ViUInt32,
    ret: *mut ViUInt32,
) -> ViStatus {
    if buf.is_null() || count == 0 {
        return VI_ERROR_INV_PARAMETER;
    }
    with_session(vi, |s| {
        // Like NI-VISA, a timeout still reports the bytes that arrived.
        let (data, code) = match s.read(count as usize) {
            Ok(read) => {
                let code = match read.reason {
                    TermReason::TermChar => VI_SUCCESS_TERM_CHAR,
                    TermReason::MaxCount => VI_SUCCESS_MAX_CNT,
                    _ => VI_SUCCESS,
                };
                (read.data, code)
            }
            Err(Error::Timeout { partial, .. }) => (partial, VI_ERROR_TMO),
            Err(e) => (Vec::new(), status(&e)),
        };
        let n = data.len().min(count as usize);
        // SAFETY: the caller passes a buffer of `count` writable bytes,
        // and `n <= count`.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), buf, n);
            set(ret, u32::try_from(n).unwrap_or(u32::MAX));
        }
        code
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viClear(vi: ViSession) -> ViStatus {
    with_session(vi, |s| result(s.clear()))
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viReadSTB(vi: ViSession, stb: *mut ViUInt16) -> ViStatus {
    with_session(vi, |s| match s.read_stb() {
        Ok(value) => {
            // SAFETY: the caller passes a valid out-pointer.
            unsafe { set(stb, value.into()) };
            VI_SUCCESS
        }
        Err(e) => status(&e),
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viAssertTrigger(vi: ViSession, protocol: ViUInt16) -> ViStatus {
    if protocol != VI_TRIG_PROT_DEFAULT {
        return VI_ERROR_INV_PROT;
    }
    with_session(vi, |s| result(s.assert_trigger()))
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viFlush(vi: ViSession, mask: ViUInt16) -> ViStatus {
    if mask == 0 {
        return VI_ERROR_INV_MASK;
    }
    with_session(vi, |_| VI_SUCCESS)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viGpibControlREN(vi: ViSession, mode: ViUInt16) -> ViStatus {
    if mode > VI_GPIB_REN_ADDRESS_GTL {
        return VI_ERROR_INV_MODE;
    }
    with_session(vi, |s| match s.resource() {
        ResourceName::GpibInstr { .. } => VI_SUCCESS,
        _ => VI_ERROR_NSUP_OPER,
    })
}

// ---- Locks ----

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viLock(
    vi: ViSession,
    mode: ViAccessMode,
    timeout_ms: ViUInt32,
    requested: *const ViChar,
    access_key: *mut ViChar,
) -> ViStatus {
    let kind = match mode {
        VI_EXCLUSIVE_LOCK => LockKind::Exclusive,
        VI_SHARED_LOCK => {
            // SAFETY: `requested` is NUL-terminated or null.
            let key = unsafe { c_str(requested) }.unwrap_or("fake-key").to_owned();
            // SAFETY: `access_key` is null or a VI_FIND_BUFLEN buffer.
            unsafe { write_str(access_key, &key) };
            LockKind::Shared { key }
        }
        _ => return VI_ERROR_INV_LOCK_TYPE,
    };
    let wait = timeout(timeout_ms).unwrap_or(Duration::from_secs(3600));
    with_session(vi, |s| match s.lock(kind, wait) {
        Ok(()) => VI_SUCCESS,
        // VISA reports a lock not acquired in time as a timeout.
        Err(Error::ResourceLocked) => VI_ERROR_TMO,
        Err(e) => status(&e),
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viUnlock(vi: ViSession) -> ViStatus {
    with_session(vi, |s| match s.unlock() {
        Ok(()) => VI_SUCCESS,
        Err(Error::InvalidArgument(_)) => VI_ERROR_SESN_NLOCKED,
        Err(e) => status(&e),
    })
}

// ---- Attributes ----

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viGetAttribute(
    vi: ViObject,
    id: ViAttr,
    out: *mut c_void,
) -> ViStatus {
    if out.is_null() {
        return VI_ERROR_INV_PARAMETER;
    }
    let Some((attr, width)) = attr(id) else {
        return VI_ERROR_NSUP_ATTR;
    };
    with_session(vi, |s| match s.get_attr(attr) {
        Ok(AttrValue::Str(text)) => {
            // SAFETY: string attributes take a VI_FIND_BUFLEN buffer.
            unsafe { write_str(out.cast(), &text) };
            VI_SUCCESS
        }
        Ok(value) => {
            let raw = to_number(&value);
            // SAFETY: the caller passes storage of the attribute's C type,
            // which is `width` bytes; the value is truncated to it.
            unsafe {
                match width {
                    1 => out.cast::<u8>().write(raw.to_le_bytes()[0]),
                    2 => out
                        .cast::<u16>()
                        .write(u16::try_from(raw & 0xFFFF).unwrap_or(0)),
                    _ => out
                        .cast::<u32>()
                        .write(u32::try_from(raw & 0xFFFF_FFFF).unwrap_or(0)),
                }
            }
            VI_SUCCESS
        }
        Err(Error::Unsupported { .. }) => VI_ERROR_NSUP_ATTR,
        Err(e) => status(&e),
    })
}

#[unsafe(no_mangle)]
#[allow(clippy::useless_conversion)] // ViAttrState is u32 on 32-bit targets
pub unsafe extern "system" fn viSetAttribute(
    vi: ViObject,
    id: ViAttr,
    value: ViAttrState,
) -> ViStatus {
    let Some((attr, _)) = attr(id) else {
        return VI_ERROR_NSUP_ATTR;
    };
    if attr.is_read_only() {
        return VI_ERROR_ATTR_READONLY;
    }
    let Some(value) = from_number(attr, value.into()) else {
        return VI_ERROR_NSUP_ATTR_STATE;
    };
    with_session(vi, |s| match s.set_attr(attr, value) {
        Ok(()) => VI_SUCCESS,
        Err(Error::Unsupported { .. }) => VI_ERROR_NSUP_ATTR,
        Err(e) => status(&e),
    })
}

// ---- Events (queue mechanism only) ----

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viEnableEvent(
    vi: ViSession,
    event: ViEventType,
    mechanism: ViUInt16,
    _filter: ViEventFilter,
) -> ViStatus {
    if mechanism != VI_QUEUE {
        return VI_ERROR_NSUP_MECH;
    }
    let Some(kind) = event_kind(event) else {
        return VI_ERROR_INV_EVENT;
    };
    with_session(vi, |s| result(s.enable_event(kind)))
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viDisableEvent(
    vi: ViSession,
    event: ViEventType,
    _mechanism: ViUInt16,
) -> ViStatus {
    let Some(kind) = event_kind(event) else {
        return VI_ERROR_INV_EVENT;
    };
    with_session(vi, |s| result(s.disable_event(kind)))
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viDiscardEvents(
    vi: ViSession,
    event: ViEventType,
    _mechanism: ViUInt16,
) -> ViStatus {
    if event_kind(event).is_none() {
        return VI_ERROR_INV_EVENT;
    }
    // `disable_event` already discards queued events in the mock.
    with_session(vi, |_| VI_SUCCESS)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn viWaitOnEvent(
    vi: ViSession,
    event: ViEventType,
    timeout_ms: ViUInt32,
    out_type: *mut ViEventType,
    out_context: *mut ViEvent,
) -> ViStatus {
    let Some(kind) = event_kind(event) else {
        return VI_ERROR_INV_EVENT;
    };
    let wait = timeout(timeout_ms).unwrap_or(Duration::from_secs(3600));
    let status = with_session(vi, |s| match s.wait_event(kind, wait) {
        Ok(_) => VI_SUCCESS,
        Err(e) => status(&e),
    });
    if status == VI_SUCCESS {
        let mut fake = state();
        let id = fake.id();
        fake.events.push(id);
        // SAFETY: the caller passes valid out-pointers (or null).
        unsafe {
            set(out_type, event);
            set(out_context, id);
        }
    }
    status
}
