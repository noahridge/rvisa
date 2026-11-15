//! The blocking VXI-11 session (DESIGN.md R4–R7).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rvisa_core::{
    Attr, AttrValue, Error, Event, EventKind, LockKind, LockRegistry, ReadResult, ResourceName,
    Result, Session, TermReason,
};
use socket2::SockRef;

use crate::proto::{
    self, DEVICE_ABORT, DEVICE_ASYNC, DEVICE_CLEAR, DEVICE_CORE, DEVICE_LOCK, DEVICE_READ,
    DEVICE_READSTB, DEVICE_TRIGGER, DEVICE_UNLOCK, DEVICE_VERSION, DEVICE_WRITE, FLAG_END,
    FLAG_TERMCHRSET, FLAG_WAITLOCK, GenericParms, LockParms, REASON_CHR, REASON_END, ReadParms,
    ReadResp, ReadStbResp, WriteParms, WriteResp, check, code, millis,
};
use crate::rpc::RpcClient;
#[cfg(not(feature = "srq"))]
use crate::settings::unsupported;
use crate::settings::{MAX_READ_REQUEST, RPC_SLACK, Settings};

/// How long closing waits for `destroy_link` and an abort for its reply.
const CLOSE_WAIT: Duration = Duration::from_secs(1);
/// How often a lock wait re-checks the in-process table.
const LOCK_POLL: Duration = Duration::from_millis(2);

/// The link a session holds, as `create_link` returned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Link {
    pub lid: i32,
    pub abort_addr: SocketAddr,
    pub max_recv_size: usize,
}

/// A blocking session on a VXI-11 link. Open it with
/// [`Vxi11Backend`](crate::Vxi11Backend).
#[derive(Debug)]
pub struct Vxi11Session {
    core: RpcClient,
    resource: ResourceName,
    link: Link,
    abort: AbortHandle,
    settings: Settings,
    locks: Arc<LockRegistry>,
    id: u64,
    /// Exclusive lock levels held; the device lock is held while this is
    /// non-zero.
    exclusive: u32,
    #[cfg(feature = "srq")]
    srq: crate::srq::Srq,
}

impl Vxi11Session {
    pub(crate) fn new(
        core: RpcClient,
        resource: ResourceName,
        link: Link,
        locks: Arc<LockRegistry>,
    ) -> Self {
        let id = locks.new_id();
        Self {
            core,
            resource,
            abort: AbortHandle::new(link.abort_addr, link.lid),
            link,
            settings: Settings::default(),
            locks,
            id,
            exclusive: 0,
            #[cfg(feature = "srq")]
            srq: crate::srq::Srq::default(),
        }
    }

    /// The link ID the device assigned.
    #[must_use]
    pub fn link_id(&self) -> i32 {
        self.link.lid
    }

    /// The largest write the device accepts in one `device_write`; longer
    /// writes are sent in parts of this size.
    #[must_use]
    pub fn max_recv_size(&self) -> usize {
        self.link.max_recv_size
    }

    /// The connected peer's address (the core channel).
    ///
    /// # Errors
    ///
    /// I/O errors from the socket.
    pub fn peer_addr(&self) -> Result<SocketAddr> {
        Ok(self.core.stream().peer_addr()?)
    }

    /// A handle that aborts this session's operation in progress from
    /// another thread, through the abort channel.
    #[must_use]
    pub fn abort_handle(&self) -> AbortHandle {
        self.abort.clone()
    }

    fn check_access(&self) -> Result<()> {
        self.locks.check(&self.resource, self.id)
    }

    /// Calls a core procedure whose device-side wait is `io_timeout`.
    /// If no reply comes within that plus [`RPC_SLACK`], the call is
    /// aborted and fails with a timeout.
    pub(crate) fn call(
        &mut self,
        proc: u32,
        args: &[u8],
        io_timeout: Option<Duration>,
    ) -> Result<Vec<u8>> {
        let wait = io_timeout.map(|t| t.saturating_add(RPC_SLACK));
        match self
            .core
            .call(DEVICE_CORE, DEVICE_VERSION, proc, args, wait)
        {
            Err(e) if e.is_timeout() => {
                tracing::debug!(resource = %self.resource, proc, "vxi11: no reply; aborting");
                if let Err(abort) = self.abort.abort() {
                    tracing::debug!(error = %abort, "vxi11: abort failed");
                }
                Err(e)
            }
            other => other,
        }
    }

    fn generic(&mut self, proc: u32) -> Result<Vec<u8>> {
        let parms = GenericParms {
            lid: self.link.lid,
            flags: 0,
            lock_timeout: 0,
            io_timeout: millis(self.settings.timeout),
        };
        self.call(proc, &parms.encode(), self.settings.timeout)
    }

    fn device_lock(&mut self, wait: Duration) -> Result<()> {
        let parms = LockParms {
            lid: self.link.lid,
            flags: FLAG_WAITLOCK,
            lock_timeout: millis(Some(wait)),
        };
        let reply = self.call(DEVICE_LOCK, &parms.encode(), Some(wait))?;
        check(proto::decode_error(&reply)?, "device_lock")
    }

    fn device_unlock(&mut self) -> Result<()> {
        let reply = self.call(
            DEVICE_UNLOCK,
            &proto::encode_link(self.link.lid),
            Some(CLOSE_WAIT),
        )?;
        check(proto::decode_error(&reply)?, "device_unlock")
    }

    /// Reads one `device_read` worth, at most `request` bytes, waiting up
    /// to `wait` on the device.
    fn device_read(&mut self, request: usize, wait: Option<Duration>) -> Result<ReadResp> {
        let term = self.settings.term();
        let parms = ReadParms {
            lid: self.link.lid,
            request_size: u32::try_from(request).unwrap_or(u32::MAX),
            io_timeout: millis(wait),
            lock_timeout: 0,
            flags: if term.is_some() { FLAG_TERMCHRSET } else { 0 },
            term_char: term.unwrap_or(self.settings.term_char),
        };
        let reply = self.call(DEVICE_READ, &parms.encode(), wait)?;
        ReadResp::decode(&reply, request)
    }
}

impl Drop for Vxi11Session {
    fn drop(&mut self) {
        #[cfg(feature = "srq")]
        crate::srq::close(self);
        // `destroy_link` releases the device lock too.
        let result = self
            .core
            .call(
                DEVICE_CORE,
                DEVICE_VERSION,
                proto::DESTROY_LINK,
                &proto::encode_link(self.link.lid),
                Some(CLOSE_WAIT),
            )
            .and_then(|reply| check(proto::decode_error(&reply)?, "destroy_link"));
        if let Err(e) = result {
            tracing::debug!(resource = %self.resource, error = %e, "vxi11: destroy_link failed");
        }
        self.locks.release_all(&self.resource, self.id);
    }
}

impl Session for Vxi11Session {
    fn resource(&self) -> &ResourceName {
        &self.resource
    }

    /// Sends `data` in `device_write` calls of at most
    /// [`max_recv_size`](Vxi11Session::max_recv_size) bytes, with END on
    /// the last one if `SendEndEnabled` (DESIGN.md R4).
    fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.check_access()?;
        if data.is_empty() {
            return Ok(0);
        }
        tracing::trace!(resource = %self.resource, len = data.len(), data = ?String::from_utf8_lossy(data), "vxi11: write");
        let chunk = self.link.max_recv_size.max(1);
        let mut sent = 0;
        while sent < data.len() {
            let part = &data[sent..data.len().min(sent + chunk)];
            let last = sent + part.len() == data.len();
            let parms = WriteParms {
                lid: self.link.lid,
                io_timeout: millis(self.settings.timeout),
                lock_timeout: 0,
                flags: if last && self.settings.send_end {
                    FLAG_END
                } else {
                    0
                },
                data: part,
            };
            let resp = self
                .call(DEVICE_WRITE, &parms.encode(), self.settings.timeout)
                .and_then(|reply| WriteResp::decode(&reply));
            let resp = match resp {
                Ok(resp) => resp,
                // A partial write is reported as such; `write_all` retries.
                Err(_) if sent > 0 => break,
                Err(e) => return Err(e),
            };
            let accepted = (resp.size as usize).min(part.len());
            sent += accepted;
            if resp.error != code::NO_ERROR {
                if sent > 0 {
                    break;
                }
                check(resp.error, "device_write")?;
            }
            if accepted < part.len() {
                break;
            }
        }
        Ok(sent)
    }

    /// Reads with `device_read` until END, the term char (if enabled) or
    /// `max` bytes (DESIGN.md R4).
    fn read(&mut self, max: usize) -> Result<ReadResult> {
        self.check_access()?;
        if max == 0 {
            return Err(Error::InvalidArgument("read of zero bytes".into()));
        }
        let deadline = self.settings.timeout.map(|t| Instant::now() + t);
        let mut data = Vec::new();
        loop {
            let wait = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            let request = (max - data.len()).min(MAX_READ_REQUEST);
            let resp = match self.device_read(request, wait) {
                Ok(resp) => resp,
                Err(e) => return Err(e.prepend_partial(data)),
            };
            data.extend_from_slice(&resp.data);
            if let Err(e) = check(resp.error, "device_read") {
                tracing::trace!(resource = %self.resource, len = data.len(), error = resp.error, "vxi11: read failed");
                // A timeout carries the bytes received before it (C12).
                return Err(e.prepend_partial(data));
            }
            let reason = if resp.reason & REASON_END != 0 {
                TermReason::End
            } else if resp.reason & REASON_CHR != 0 {
                TermReason::TermChar
            } else if data.len() >= max {
                TermReason::MaxCount
            } else if deadline.is_some_and(|d| Instant::now() >= d) {
                return Err(Error::read_timeout(data));
            } else {
                // The device sent less than asked without a reason (some
                // limit their replies to a buffer size): ask for the rest.
                continue;
            };
            tracing::trace!(resource = %self.resource, len = data.len(), ?reason, "vxi11: read");
            return Ok(ReadResult { data, reason });
        }
    }

    fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<()> {
        self.settings.timeout = timeout;
        Ok(())
    }

    fn timeout(&self) -> Option<Duration> {
        self.settings.timeout
    }

    fn set_read_termination(&mut self, term: Option<u8>) -> Result<()> {
        self.settings.set_read_termination(term);
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        self.check_access()?;
        let reply = self.generic(DEVICE_CLEAR)?;
        check(proto::decode_error(&reply)?, "device_clear")
    }

    fn read_stb(&mut self) -> Result<u8> {
        self.check_access()?;
        let reply = self.generic(DEVICE_READSTB)?;
        let resp = ReadStbResp::decode(&reply)?;
        check(resp.error, "device_readstb")?;
        Ok(resp.stb)
    }

    fn assert_trigger(&mut self) -> Result<()> {
        self.check_access()?;
        let reply = self.generic(DEVICE_TRIGGER)?;
        check(proto::decode_error(&reply)?, "device_trigger")
    }

    /// Locks in process (DESIGN.md C13); the first level of an exclusive
    /// lock also takes the device lock, which keeps other clients out
    /// (DESIGN.md R6).
    fn lock(&mut self, kind: LockKind, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.locks.try_lock(&self.resource, self.id, &kind) {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(Error::ResourceLocked);
            }
            std::thread::sleep(LOCK_POLL.min(deadline - now));
        }
        if kind == LockKind::Exclusive {
            if self.exclusive == 0
                && let Err(e) = self.device_lock(deadline.saturating_duration_since(Instant::now()))
            {
                // Undo the level just taken in process.
                let _ = self.locks.unlock(&self.resource, self.id);
                return Err(e);
            }
            self.exclusive += 1;
        }
        Ok(())
    }

    fn unlock(&mut self) -> Result<()> {
        // `LockTable::unlock` releases an exclusive level before a shared
        // one, so the counts stay in step.
        self.locks.unlock(&self.resource, self.id)?;
        if self.exclusive > 0 {
            self.exclusive -= 1;
            if self.exclusive == 0 {
                self.device_unlock()?;
            }
        }
        Ok(())
    }

    fn get_attr(&self, attr: Attr) -> Result<AttrValue> {
        if let Some(value) = self.settings.get(attr, &self.resource)? {
            return Ok(value);
        }
        let stream = self.core.stream();
        Ok(AttrValue::Bool(match attr {
            Attr::TcpipNodelay => stream.nodelay()?,
            _ => SockRef::from(stream).keepalive()?,
        }))
    }

    fn set_attr(&mut self, attr: Attr, value: AttrValue) -> Result<()> {
        if self.settings.set(attr, &value)? {
            return Ok(());
        }
        let on = value.as_bool().unwrap_or_default();
        let stream = self.core.stream();
        match attr {
            Attr::TcpipNodelay => stream.set_nodelay(on)?,
            _ => SockRef::from(stream).set_keepalive(on)?,
        }
        Ok(())
    }

    #[cfg(feature = "srq")]
    fn enable_event(&mut self, kind: EventKind) -> Result<()> {
        crate::srq::enable(self, kind)
    }

    #[cfg(feature = "srq")]
    fn wait_event(&mut self, kind: EventKind, timeout: Duration) -> Result<Event> {
        crate::srq::wait(self, kind, timeout)
    }

    #[cfg(feature = "srq")]
    fn disable_event(&mut self, kind: EventKind) -> Result<()> {
        crate::srq::disable(self, kind)
    }

    /// Without the `srq` feature there are no events (DESIGN.md R7).
    #[cfg(not(feature = "srq"))]
    fn enable_event(&mut self, _kind: EventKind) -> Result<()> {
        Err(unsupported("enable_event"))
    }

    #[cfg(not(feature = "srq"))]
    fn wait_event(&mut self, _kind: EventKind, _timeout: Duration) -> Result<Event> {
        Err(unsupported("wait_event"))
    }

    #[cfg(not(feature = "srq"))]
    fn disable_event(&mut self, _kind: EventKind) -> Result<()> {
        Err(unsupported("disable_event"))
    }
}

#[cfg(feature = "srq")]
impl Vxi11Session {
    pub(crate) fn srq_state(&mut self) -> &mut crate::srq::Srq {
        &mut self.srq
    }

    pub(crate) fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.core.stream().local_addr()?)
    }

    pub(crate) fn lid(&self) -> i32 {
        self.link.lid
    }

    pub(crate) fn resource_name(&self) -> &ResourceName {
        &self.resource
    }
}

/// Aborts a session's operation in progress through the VXI-11 abort
/// channel (`device_abort`). It can be used from another thread while the
/// session is blocked in a read or write; the blocked call then fails.
///
/// The abort channel is connected on first use.
#[derive(Debug, Clone)]
pub struct AbortHandle {
    addr: SocketAddr,
    lid: i32,
    conn: Arc<Mutex<Option<RpcClient>>>,
}

impl AbortHandle {
    fn new(addr: SocketAddr, lid: i32) -> Self {
        Self {
            addr,
            lid,
            conn: Arc::default(),
        }
    }

    /// Sends `device_abort` for the session's link.
    ///
    /// # Errors
    ///
    /// I/O errors connecting to the abort channel, a timeout if the device
    /// does not answer within a second, or the device's error.
    pub fn abort(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        let client = match &mut *conn {
            Some(client) => client,
            slot @ None => slot.insert(RpcClient::connect(self.addr, CLOSE_WAIT, 64)?),
        };
        let result = client
            .call(
                DEVICE_ASYNC,
                DEVICE_VERSION,
                DEVICE_ABORT,
                &proto::encode_link(self.lid),
                Some(CLOSE_WAIT),
            )
            .and_then(|reply| check(proto::decode_error(&reply)?, "device_abort"));
        if matches!(result, Err(Error::Io(_))) {
            *conn = None; // reconnect next time
        }
        result
    }
}
