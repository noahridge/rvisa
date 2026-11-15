//! A VXI-11 instrument served on a local port (`server` feature,
//! DESIGN.md R9): the backend's test peer.

use std::collections::HashMap;
use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rvisa_core::{Error, EventKind, LockKind, ResourceName, Session, TermReason};

use crate::proto::{
    self, CREATE_INTR_CHAN, CREATE_LINK, CreateLinkParms, CreateLinkResp, DESTROY_INTR_CHAN,
    DESTROY_LINK, DEVICE_ABORT, DEVICE_ASYNC, DEVICE_CLEAR, DEVICE_CORE, DEVICE_ENABLE_SRQ,
    DEVICE_INTR, DEVICE_INTR_SRQ, DEVICE_LOCAL, DEVICE_LOCK, DEVICE_READ, DEVICE_READSTB,
    DEVICE_REMOTE, DEVICE_TRIGGER, DEVICE_UNLOCK, DEVICE_VERSION, DEVICE_WRITE, EnableSrqParms,
    FLAG_END, FLAG_TERMCHRSET, FLAG_WAITLOCK, GenericParms, LockParms, PMAPPROC_GETPORT,
    PORTMAP_PROG, PORTMAP_VERS, REASON_CHR, REASON_END, REASON_REQCNT, ReadParms, ReadResp,
    ReadStbResp, RemoteFunc, WriteParms, WriteResp, code,
};
use crate::rpc::{AcceptStat, Call, RecordReader, decode_call, encode_call, encode_reply, frame};

const POLL: Duration = Duration::from_millis(2);
/// The largest call accepted.
const MAX_CALL: usize = 64 * 1024 * 1024;
/// The most one `device_read` returns.
const MAX_READ: usize = 1024 * 1024;
/// The wait standing in for an infinite lock timeout.
const FOREVER: Duration = Duration::from_secs(24 * 60 * 60);

type Opener = dyn Fn(&str) -> rvisa_core::Result<Box<dyn Session>> + Send + Sync;

/// Serves sessions as a VXI-11 instrument on `127.0.0.1`: each link
/// (`create_link`) gets a fresh session from a function given the device
/// name, and the VXI-11 calls are mapped onto it. A message written in
/// several parts reaches the session as one write, when END arrives.
///
/// One port answers both the portmapper (TCP and UDP) and the core
/// channel, so point the client at it with
/// [`Vxi11Backend::portmapper_port`](crate::Vxi11Backend::portmapper_port).
/// The abort channel has a port of its own. Service requests the session
/// raises go back over the interrupt channel. The server stops when this
/// value is dropped.
///
/// ```
/// use rvisa_core::{Backend, OpenOptions, SessionExt};
/// use rvisa_mock::{MockBackend, Script};
/// use rvisa_vxi11::{Vxi11Backend, Vxi11Server};
///
/// let rn: rvisa_core::ResourceName = "TCPIP0::sim::INSTR".parse()?;
/// let mock = MockBackend::new()
///     .instrument(rn.clone(), Script::new().on_write("*IDN?\n").respond(b"ACME,1,2,3\n"));
/// let server = Vxi11Server::spawn(move |_device| mock.open(&rn, &OpenOptions::new()))?;
///
/// let backend = Vxi11Backend::new().portmapper_port(server.port());
/// let mut s = backend.open(server.resource(), &OpenOptions::new())?;
/// assert_eq!(s.query_bytes(b"*IDN?\n")?, b"ACME,1,2,3\n");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Vxi11Server {
    addr: SocketAddr,
    resource: ResourceName,
    stats: Arc<Stats>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for Vxi11Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vxi11Server")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct Stats {
    aborts: AtomicU32,
    max_recv_size: AtomicU32,
    writes: Mutex<Vec<usize>>,
}

struct Shared {
    open: Box<Opener>,
    stats: Arc<Stats>,
    abort_port: u16,
    port: u16,
    next_lid: AtomicI32,
    stop: Arc<AtomicBool>,
}

impl Vxi11Server {
    /// Starts serving on free local ports, with a `maxRecvSize` of 64 KiB.
    ///
    /// # Errors
    ///
    /// If a socket cannot be bound.
    pub fn spawn(
        open: impl Fn(&str) -> rvisa_core::Result<Box<dyn Session>> + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let udp = UdpSocket::bind(addr)?;
        udp.set_read_timeout(Some(POLL))?;
        let abort = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        abort.set_nonblocking(true)?;
        let resource: ResourceName = format!("TCPIP0::{}::inst0::INSTR", addr.ip())
            .parse()
            .map_err(io::Error::other)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Stats {
            max_recv_size: AtomicU32::new(64 * 1024),
            ..Stats::default()
        });
        let shared = Arc::new(Shared {
            open: Box::new(open),
            stats: Arc::clone(&stats),
            abort_port: abort.local_addr()?.port(),
            port: addr.port(),
            next_lid: AtomicI32::new(1),
            stop: Arc::clone(&stop),
        });
        let threads = vec![
            {
                let shared = Arc::clone(&shared);
                thread::spawn(move || accept_loop(&listener, &shared, serve_core))
            },
            {
                let shared = Arc::clone(&shared);
                thread::spawn(move || accept_loop(&abort, &shared, serve_abort))
            },
            {
                let shared = Arc::clone(&shared);
                thread::spawn(move || serve_udp(&udp, &shared))
            },
        ];
        Ok(Self {
            addr,
            resource,
            stats,
            stop,
            threads,
        })
    }

    /// Sets the `maxRecvSize` reported to new links.
    #[must_use]
    pub fn max_recv_size(self, size: u32) -> Self {
        self.stats.max_recv_size.store(size, Ordering::Relaxed);
        self
    }

    /// The port of the portmapper and core channel.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// The core channel's address.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `TCPIP0::127.0.0.1::inst0::INSTR`.
    #[must_use]
    pub fn resource(&self) -> &ResourceName {
        &self.resource
    }

    /// How many `device_abort` calls arrived.
    #[must_use]
    pub fn aborts(&self) -> u32 {
        self.stats.aborts.load(Ordering::Relaxed)
    }

    /// The size of every `device_write` received, in order.
    #[must_use]
    pub fn write_sizes(&self) -> Vec<usize> {
        self.stats
            .writes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Vxi11Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn accept_loop(
    listener: &TcpListener,
    shared: &Arc<Shared>,
    serve: fn(TcpStream, &Shared) -> io::Result<()>,
) {
    let mut connections = Vec::new();
    while !shared.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                tracing::debug!(%peer, "vxi11 server: connection");
                let shared = Arc::clone(shared);
                connections.push(thread::spawn(move || {
                    if let Err(e) = serve(stream, &shared) {
                        tracing::debug!(error = %e, "vxi11 server: connection ended");
                    }
                }));
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(e) => {
                tracing::debug!(error = %e, "vxi11 server: accept failed");
                break;
            }
        }
    }
    for connection in connections {
        let _ = connection.join();
    }
}

/// Reads calls from `stream` and answers each with `handle`.
fn pump(
    mut stream: TcpStream,
    shared: &Shared,
    mut handle: impl FnMut(&Call<'_>) -> (AcceptStat, Vec<u8>),
    mut idle: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(POLL))?;
    stream.set_nodelay(true)?;
    let mut reader = RecordReader::default();
    while !shared.stop.load(Ordering::Relaxed) {
        let Some(record) = reader.next(&mut stream, MAX_CALL)? else {
            idle()?;
            continue;
        };
        let call = decode_call(&record)
            .map_err(|e| io::Error::new(ErrorKind::InvalidData, e.to_string()))?;
        let (stat, results) = handle(&call);
        crate::rpc::send(&mut stream, &frame(&encode_reply(call.xid, stat, &results)))
            .map_err(io::Error::other)?;
    }
    Ok(())
}

fn serve_abort(stream: TcpStream, shared: &Shared) -> io::Result<()> {
    pump(
        stream,
        shared,
        |call| match (call.prog, call.proc) {
            (DEVICE_ASYNC, DEVICE_ABORT) => {
                shared.stats.aborts.fetch_add(1, Ordering::Relaxed);
                (AcceptStat::Success, proto::encode_error(code::NO_ERROR))
            }
            (DEVICE_ASYNC, 0) => (AcceptStat::Success, Vec::new()),
            (DEVICE_ASYNC, _) => (AcceptStat::ProcUnavail, Vec::new()),
            _ => (AcceptStat::ProgUnavail, Vec::new()),
        },
        || Ok(()),
    )
}

/// The portmapper's answers: this server's ports.
fn portmap(shared: &Shared, call: &Call<'_>) -> (AcceptStat, Vec<u8>) {
    match call.proc {
        0 => (AcceptStat::Success, Vec::new()),
        PMAPPROC_GETPORT => match proto::decode_getport(call.args) {
            Ok((prog, _, _)) => {
                let port = match prog {
                    DEVICE_CORE => shared.port,
                    DEVICE_ASYNC => shared.abort_port,
                    _ => 0,
                };
                (AcceptStat::Success, proto::encode_port(port))
            }
            Err(_) => (AcceptStat::GarbageArgs, Vec::new()),
        },
        _ => (AcceptStat::ProcUnavail, Vec::new()),
    }
}

fn serve_udp(socket: &UdpSocket, shared: &Shared) {
    let mut buf = [0u8; 1024];
    while !shared.stop.load(Ordering::Relaxed) {
        let Ok((n, from)) = socket.recv_from(&mut buf) else {
            continue;
        };
        let Ok(call) = decode_call(&buf[..n]) else {
            continue;
        };
        if call.prog == PORTMAP_PROG && call.vers == PORTMAP_VERS {
            let (stat, results) = portmap(shared, &call);
            let _ = socket.send_to(&encode_reply(call.xid, stat, &results), from);
        }
    }
}

/// One link's state.
struct LinkState {
    session: Box<dyn Session>,
    /// A message received without END so far.
    message: Vec<u8>,
    srq: Option<Vec<u8>>,
}

/// One core connection's state.
struct Core<'a> {
    shared: &'a Shared,
    links: HashMap<i32, LinkState>,
    intr: Option<(TcpStream, u32)>,
}

fn serve_core(stream: TcpStream, shared: &Shared) -> io::Result<()> {
    let core = std::cell::RefCell::new(Core {
        shared,
        links: HashMap::new(),
        intr: None,
    });
    pump(
        stream,
        shared,
        |call| {
            if call.prog == PORTMAP_PROG {
                return portmap(shared, call);
            }
            if call.prog != DEVICE_CORE {
                return (AcceptStat::ProgUnavail, Vec::new());
            }
            if call.vers != DEVICE_VERSION {
                let range = crate::xdr::Encoder::new().u32(1).u32(1).finish();
                return (AcceptStat::ProgMismatch, range);
            }
            match core.borrow_mut().call(call) {
                Some(results) => (AcceptStat::Success, results),
                None => (AcceptStat::GarbageArgs, Vec::new()),
            }
        },
        || {
            core.borrow_mut().forward_srqs();
            Ok(())
        },
    )
}

/// A VXI-11 error code for a session error.
fn error_code(e: &Error) -> i32 {
    match e {
        Error::ResourceLocked => code::LOCKED,
        Error::Timeout { .. } => code::IO_TIMEOUT,
        Error::Unsupported { .. } => code::NOT_SUPPORTED,
        Error::NotFound(_) => code::INVALID_ADDRESS,
        Error::InvalidArgument(_) => code::PARAMETER_ERROR,
        _ => code::IO_ERROR,
    }
}

fn timeout(ms: u32) -> Option<Duration> {
    (ms != u32::MAX).then(|| Duration::from_millis(u64::from(ms)))
}

impl Core<'_> {
    /// Handles a core channel call; `None` for undecodable arguments.
    fn call(&mut self, call: &Call<'_>) -> Option<Vec<u8>> {
        let args = call.args;
        Some(match call.proc {
            0 => Vec::new(),
            CREATE_LINK => self.create_link(&CreateLinkParms::decode(args).ok()?),
            DEVICE_WRITE => self.write(&WriteParms::decode(args, MAX_CALL).ok()?),
            DEVICE_READ => self.read(&ReadParms::decode(args).ok()?),
            DEVICE_READSTB => {
                let p = GenericParms::decode(args).ok()?;
                let resp = match self.link(p.lid).map(|l| l.session.read_stb()) {
                    Ok(Ok(stb)) => ReadStbResp { error: 0, stb },
                    Ok(Err(e)) => ReadStbResp {
                        error: error_code(&e),
                        stb: 0,
                    },
                    Err(error) => ReadStbResp { error, stb: 0 },
                };
                resp.encode()
            }
            DEVICE_TRIGGER | DEVICE_CLEAR | DEVICE_REMOTE | DEVICE_LOCAL => {
                let p = GenericParms::decode(args).ok()?;
                let proc = call.proc;
                proto::encode_error(self.with_link(p.lid, |l| match proc {
                    DEVICE_TRIGGER => l.session.assert_trigger(),
                    DEVICE_CLEAR => {
                        l.message.clear();
                        l.session.clear()
                    }
                    _ => Ok(()),
                }))
            }
            DEVICE_LOCK => {
                let p = LockParms::decode(args).ok()?;
                let wait = if p.flags & FLAG_WAITLOCK != 0 {
                    timeout(p.lock_timeout).unwrap_or(FOREVER)
                } else {
                    Duration::ZERO
                };
                proto::encode_error(
                    self.with_link(p.lid, |l| l.session.lock(LockKind::Exclusive, wait)),
                )
            }
            DEVICE_UNLOCK => {
                let lid = proto::decode_link(args).ok()?;
                proto::encode_error(match self.link(lid) {
                    Ok(l) => match l.session.unlock() {
                        Ok(()) => code::NO_ERROR,
                        Err(_) => code::NO_LOCK_HELD,
                    },
                    Err(error) => error,
                })
            }
            DEVICE_ENABLE_SRQ => {
                let p = EnableSrqParms::decode(args).ok()?;
                proto::encode_error(self.with_link(p.lid, |l| {
                    if p.enable {
                        l.session.enable_event(EventKind::ServiceRequest)?;
                        l.srq = Some(p.handle.clone());
                    } else {
                        l.srq = None;
                        l.session.disable_event(EventKind::ServiceRequest)?;
                    }
                    Ok(())
                }))
            }
            DESTROY_LINK => {
                let lid = proto::decode_link(args).ok()?;
                let error = if self.links.remove(&lid).is_some() {
                    code::NO_ERROR
                } else {
                    code::INVALID_LINK
                };
                proto::encode_error(error)
            }
            CREATE_INTR_CHAN => {
                let func = RemoteFunc::decode(args).ok()?;
                proto::encode_error(self.create_intr_chan(&func))
            }
            DESTROY_INTR_CHAN => proto::encode_error(if self.intr.take().is_some() {
                code::NO_ERROR
            } else {
                code::NO_CHANNEL
            }),
            _ => return None,
        })
    }

    fn link(&mut self, lid: i32) -> Result<&mut LinkState, i32> {
        self.links.get_mut(&lid).ok_or(code::INVALID_LINK)
    }

    fn with_link(
        &mut self,
        lid: i32,
        f: impl FnOnce(&mut LinkState) -> rvisa_core::Result<()>,
    ) -> i32 {
        match self.link(lid) {
            Ok(link) => match f(link) {
                Ok(()) => code::NO_ERROR,
                Err(e) => error_code(&e),
            },
            Err(error) => error,
        }
    }

    fn create_link(&mut self, p: &CreateLinkParms) -> Vec<u8> {
        let max_recv_size = self.shared.stats.max_recv_size.load(Ordering::Relaxed);
        let mut resp = CreateLinkResp {
            error: code::NO_ERROR,
            lid: 0,
            abort_port: self.shared.abort_port,
            max_recv_size,
        };
        match (self.shared.open)(&p.device) {
            Ok(mut session) => {
                if p.lock_device
                    && let Err(e) = session.lock(
                        LockKind::Exclusive,
                        timeout(p.lock_timeout).unwrap_or(FOREVER),
                    )
                {
                    resp.error = error_code(&e);
                    return resp.encode();
                }
                let lid = self.shared.next_lid.fetch_add(1, Ordering::Relaxed);
                self.links.insert(
                    lid,
                    LinkState {
                        session,
                        message: Vec::new(),
                        srq: None,
                    },
                );
                resp.lid = lid;
            }
            Err(e) => resp.error = error_code(&e),
        }
        resp.encode()
    }

    fn write(&mut self, p: &WriteParms<'_>) -> Vec<u8> {
        self.shared
            .stats
            .writes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(p.data.len());
        let size = u32::try_from(p.data.len()).unwrap_or(u32::MAX);
        let error = self.with_link(p.lid, |l| {
            l.message.extend_from_slice(p.data);
            if p.flags & FLAG_END != 0 {
                l.session.set_timeout(timeout(p.io_timeout))?;
                let message = std::mem::take(&mut l.message);
                // Mock sessions match a whole message per write.
                l.session.write(&message)?;
            }
            Ok(())
        });
        WriteResp {
            error,
            size: if error == code::NO_ERROR { size } else { 0 },
        }
        .encode()
    }

    fn read(&mut self, p: &ReadParms) -> Vec<u8> {
        let resp = match self.link(p.lid) {
            Err(error) => ReadResp {
                error,
                reason: 0,
                data: Vec::new(),
            },
            Ok(l) => {
                let term = (p.flags & FLAG_TERMCHRSET != 0).then_some(p.term_char);
                let max = (p.request_size as usize).clamp(1, MAX_READ);
                let result = l
                    .session
                    .set_timeout(timeout(p.io_timeout))
                    .and_then(|()| l.session.set_read_termination(term))
                    .and_then(|()| l.session.read(max));
                match result {
                    Ok(read) => ReadResp {
                        error: code::NO_ERROR,
                        reason: match read.reason {
                            TermReason::End => REASON_END,
                            TermReason::TermChar => REASON_CHR,
                            _ => REASON_REQCNT,
                        },
                        data: read.data,
                    },
                    Err(Error::Timeout { partial, .. }) => ReadResp {
                        error: code::IO_TIMEOUT,
                        reason: 0,
                        data: partial,
                    },
                    Err(e) => ReadResp {
                        error: error_code(&e),
                        reason: 0,
                        data: Vec::new(),
                    },
                }
            }
        };
        resp.encode()
    }

    fn create_intr_chan(&mut self, func: &RemoteFunc) -> i32 {
        if self.intr.is_some() {
            return code::CHANNEL_ESTABLISHED;
        }
        let addr = SocketAddr::from((Ipv4Addr::from(func.host_addr), func.host_port));
        match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                self.intr = Some((stream, 1));
                code::NO_ERROR
            }
            Err(e) => {
                tracing::debug!(%addr, error = %e, "vxi11 server: interrupt channel failed");
                code::NO_CHANNEL
            }
        }
    }

    /// Sends `device_intr_srq` for each link whose session raised one.
    fn forward_srqs(&mut self) {
        let Some((stream, xid)) = &mut self.intr else {
            return;
        };
        for link in self.links.values_mut() {
            let Some(handle) = &link.srq else { continue };
            if link
                .session
                .wait_event(EventKind::ServiceRequest, Duration::ZERO)
                .is_ok()
            {
                let call = encode_call(
                    *xid,
                    DEVICE_INTR,
                    DEVICE_VERSION,
                    DEVICE_INTR_SRQ,
                    &proto::encode_srq_handle(handle),
                );
                *xid = xid.wrapping_add(1);
                if let Err(e) = crate::rpc::send(stream, &frame(&call)) {
                    tracing::debug!(error = %e, "vxi11 server: SRQ not delivered");
                }
            }
        }
        // Replies to the SRQ calls are not needed; discard them.
        let _ = stream.set_nonblocking(true);
        let mut sink = [0u8; 256];
        while matches!(io::Read::read(stream, &mut sink), Ok(n) if n > 0) {}
        let _ = stream.set_nonblocking(false);
    }
}
