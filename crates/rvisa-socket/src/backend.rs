//! The socket backend: options, connecting, and the in-process lock
//! registry its sessions share (DESIGN.md S1, S6).

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rvisa_core::{
    Backend, Error, LockKind, LockTable, OpenOptions, Pattern, ResourceName, Result, Session,
};

use crate::BACKEND;
use crate::session::SocketSession;
use crate::settings::Options;

/// The pure-Rust backend for raw TCP socket resources
/// (`TCPIP[board]::host::port::SOCKET`).
///
/// Its defaults match NI-VISA: a 2 s I/O timeout, the term char `\n` but
/// disabled, `TCP_NODELAY` and keepalive on (DESIGN.md S1). Most SCPI
/// instruments need the term char enabled, since a raw socket has no other
/// way to end a message:
///
/// ```no_run
/// use rvisa_core::{Backend, OpenOptions, Session, SessionExt};
/// use rvisa_socket::SocketBackend;
///
/// let backend = SocketBackend::new();
/// let mut dmm = backend.open(&"TCPIP0::192.168.1.50::5025::SOCKET".parse()?, &OpenOptions::new())?;
/// dmm.set_read_termination(Some(b'\n'))?;
/// let idn = dmm.query_bytes(b"*IDN?\n")?;
/// # Ok::<(), rvisa_core::Error>(())
/// ```
#[derive(Debug, Clone, Default)]
pub struct SocketBackend {
    pub(crate) options: Options,
    pub(crate) locks: Arc<LockRegistry>,
}

impl SocketBackend {
    /// A backend with the default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How long `open` waits for the TCP connection (5 s by default).
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.options.connect_timeout = timeout;
        self
    }

    /// Sets `TCP_NODELAY` on new sessions (on by default).
    #[must_use]
    pub fn nodelay(mut self, on: bool) -> Self {
        self.options.nodelay = on;
        self
    }

    /// Sets TCP keepalive on new sessions (on by default).
    #[must_use]
    pub fn keepalive(mut self, on: bool) -> Self {
        self.options.keepalive = on;
        self
    }

    /// Emulates `read_stb` with `*STB?` and `assert_trigger` with `*TRG` on
    /// new sessions, like NI-VISA's `VI_PROT_4882_STRS` (DESIGN.md S5).
    /// Off by default: both then return `Unsupported`. Sessions can change
    /// it with [`Attr::Raw`](rvisa_core::Attr::Raw)`(`[`ATTR_IO_PROT`](crate::ATTR_IO_PROT)`)`.
    #[must_use]
    pub fn emulate_488_strings(mut self, on: bool) -> Self {
        self.options.strings = on;
        self
    }

    /// Bytes to send after a device clear has drained the input, such as a
    /// vendor's abort command (DESIGN.md S4). None by default.
    #[must_use]
    pub fn clear_command(mut self, command: impl AsRef<[u8]>) -> Self {
        self.options.clear_command = Some(command.as_ref().to_vec());
        self
    }

    /// Opens a session as the concrete [`SocketSession`].
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] for a resource that is not a socket;
    /// [`Error::NotFound`] if the host does not resolve or refuses the
    /// connection; [`Error::Timeout`] if connecting takes longer than
    /// [`connect_timeout`](Self::connect_timeout); [`Error::ResourceLocked`]
    /// if a lock requested in `options` is not granted in time; other I/O
    /// errors.
    pub fn open_socket(
        &self,
        resource: &ResourceName,
        options: &OpenOptions,
    ) -> Result<SocketSession> {
        let ResourceName::TcpipSocket { host, port, .. } = resource else {
            return Err(Error::InvalidArgument(format!(
                "{resource} is not a SOCKET resource"
            )));
        };
        let addrs: Vec<SocketAddr> = (host.as_str(), *port)
            .to_socket_addrs()
            .map_err(|e| {
                tracing::debug!(%resource, error = %e, "socket: cannot resolve host");
                Error::NotFound(resource.clone())
            })?
            .collect();
        let stream = connect(&addrs, self.options.connect_timeout, resource)?;
        let mut session = SocketSession::new(
            stream,
            resource.clone(),
            &self.options,
            Arc::clone(&self.locks),
        )?;
        if let Some(kind) = &options.lock {
            session.lock(kind.clone(), options.open_timeout)?;
        }
        Ok(session)
    }
}

/// Connects to the first address that accepts.
fn connect(addrs: &[SocketAddr], timeout: Duration, resource: &ResourceName) -> Result<TcpStream> {
    let mut last = None;
    for addr in addrs {
        tracing::debug!(%resource, %addr, "socket: connecting");
        match TcpStream::connect_timeout(addr, timeout.max(Duration::from_millis(1))) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
            Error::timeout()
        }
        Some(e) if e.kind() == ErrorKind::ConnectionRefused => Error::NotFound(resource.clone()),
        Some(e) => Error::Io(e),
        None => Error::NotFound(resource.clone()),
    })
}

impl Backend for SocketBackend {
    fn name(&self) -> &'static str {
        BACKEND
    }

    fn supports(&self, resource: &ResourceName) -> bool {
        matches!(resource, ResourceName::TcpipSocket { .. })
    }

    fn open(&self, resource: &ResourceName, options: &OpenOptions) -> Result<Box<dyn Session>> {
        Ok(Box::new(self.open_socket(resource, options)?))
    }

    /// Raw sockets cannot be discovered, so this is always empty
    /// (DESIGN.md S1). The pattern is still checked.
    fn list(&self, pattern: &str) -> Result<Vec<ResourceName>> {
        Pattern::new(pattern)?;
        Ok(Vec::new())
    }
}

/// Lock tables per resource, shared by one backend's sessions
/// (DESIGN.md C13, S6).
#[derive(Debug, Default)]
pub(crate) struct LockRegistry {
    tables: Mutex<HashMap<ResourceName, LockTable>>,
    next_id: AtomicU64,
}

impl LockRegistry {
    /// A new session ID.
    pub fn new_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn with<R>(&self, resource: &ResourceName, f: impl FnOnce(&mut LockTable) -> R) -> R {
        let mut tables = self.tables.lock().unwrap_or_else(PoisonError::into_inner);
        let table = tables.entry(resource.clone()).or_default();
        let result = f(table);
        if *table == LockTable::new() {
            tables.remove(resource);
        }
        result
    }

    /// `Err(ResourceLocked)` if another session's lock keeps `id` out.
    pub fn check(&self, resource: &ResourceName, id: u64) -> Result<()> {
        if self.with(resource, |t| t.blocks(id)) {
            Err(Error::ResourceLocked)
        } else {
            Ok(())
        }
    }

    pub fn try_lock(&self, resource: &ResourceName, id: u64, kind: &LockKind) -> bool {
        self.with(resource, |t| t.try_lock(id, kind))
    }

    pub fn unlock(&self, resource: &ResourceName, id: u64) -> Result<()> {
        if self.with(resource, |t| t.unlock(id)) {
            Ok(())
        } else {
            Err(Error::InvalidArgument("the session holds no lock".into()))
        }
    }

    pub fn release_all(&self, resource: &ResourceName, id: u64) {
        self.with(resource, |t| t.release_all(id));
    }
}

/// How often a lock wait re-checks the table.
pub(crate) const LOCK_POLL: Duration = Duration::from_millis(2);
