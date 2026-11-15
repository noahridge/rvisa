//! Service requests through the VXI-11 interrupt channel (`srq` feature,
//! DESIGN.md R7).
//!
//! The session serves the interrupt channel itself: it listens on the
//! local address of its core connection, asks the device to connect back
//! (`create_intr_chan`), and counts the `device_intr_srq` calls that carry
//! its handle.

use std::io::ErrorKind;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rvisa_core::{Error, Event, EventKind, Result};

use crate::proto::{
    self, CREATE_INTR_CHAN, DESTROY_INTR_CHAN, DEVICE_ENABLE_SRQ, DEVICE_INTR, DEVICE_INTR_SRQ,
    DEVICE_VERSION, EnableSrqParms, RemoteFunc, check,
};
use crate::rpc::{AcceptStat, RecordReader, decode_call, encode_reply, frame, send};
use crate::session::Vxi11Session;
use crate::settings::unsupported;

/// How long setting up or tearing down the channel waits for the device.
const SETUP_WAIT: Duration = Duration::from_secs(2);
/// How often the server thread polls its sockets.
const POLL: Duration = Duration::from_millis(2);
/// `DEVICE_TCP`.
const FAMILY_TCP: u32 = 0;

/// A session's SRQ state.
#[derive(Debug, Default)]
pub(crate) struct Srq {
    enabled: bool,
    channel: Option<Channel>,
}

/// SRQs received and not yet waited for.
#[derive(Debug, Default)]
struct Pending {
    count: Mutex<u32>,
    arrived: Condvar,
}

/// The interrupt channel server.
#[derive(Debug)]
struct Channel {
    pending: Arc<Pending>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn handle(session: &Vxi11Session) -> Vec<u8> {
    session.lid().to_be_bytes().to_vec()
}

/// Starts the interrupt server and asks the device to connect to it.
fn open_channel(session: &mut Vxi11Session) -> Result<Channel> {
    let local = session.local_addr()?;
    let IpAddr::V4(ip) = local.ip() else {
        // `Device_RemoteFunc` carries an IPv4 address only.
        return Err(unsupported("service requests over IPv6"));
    };
    let listener = TcpListener::bind((ip, 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let pending = Arc::new(Pending::default());
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let (pending, stop, handle) = (Arc::clone(&pending), Arc::clone(&stop), handle(session));
        thread::spawn(move || serve(&listener, &pending, &stop, &handle))
    };
    let channel = Channel {
        pending,
        stop,
        thread: Some(thread),
    };
    let func = RemoteFunc {
        host_addr: u32::from(ip),
        host_port: port,
        prog_num: DEVICE_INTR,
        prog_vers: DEVICE_VERSION,
        prog_family: FAMILY_TCP,
    };
    tracing::debug!(resource = %session.resource_name(), %ip, port, "vxi11: creating the interrupt channel");
    let reply = session.call(CREATE_INTR_CHAN, &func.encode(), Some(SETUP_WAIT))?;
    check(proto::decode_error(&reply)?, "create_intr_chan")?;
    Ok(channel)
}

fn enable_srq(session: &mut Vxi11Session, enable: bool) -> Result<()> {
    let parms = EnableSrqParms {
        lid: session.lid(),
        enable,
        handle: handle(session),
    };
    let reply = session.call(DEVICE_ENABLE_SRQ, &parms.encode(), Some(SETUP_WAIT))?;
    check(proto::decode_error(&reply)?, "device_enable_srq")
}

pub(crate) fn enable(session: &mut Vxi11Session, kind: EventKind) -> Result<()> {
    if kind != EventKind::ServiceRequest {
        return Err(unsupported("enable_event"));
    }
    if session.srq_state().channel.is_none() {
        let channel = open_channel(session)?;
        session.srq_state().channel = Some(channel);
    }
    enable_srq(session, true)?;
    session.srq_state().enabled = true;
    Ok(())
}

pub(crate) fn wait(
    session: &mut Vxi11Session,
    kind: EventKind,
    timeout: Duration,
) -> Result<Event> {
    if kind != EventKind::ServiceRequest {
        return Err(unsupported("wait_event"));
    }
    let srq = session.srq_state();
    let (true, Some(channel)) = (srq.enabled, &srq.channel) else {
        return Err(Error::EventNotEnabled(kind));
    };
    let deadline = Instant::now() + timeout;
    let pending = &channel.pending;
    let mut count = pending.count.lock().unwrap_or_else(PoisonError::into_inner);
    while *count == 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Error::timeout());
        }
        count = pending
            .arrived
            .wait_timeout(count, left)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
    *count -= 1;
    Ok(Event::new(kind))
}

pub(crate) fn disable(session: &mut Vxi11Session, kind: EventKind) -> Result<()> {
    if kind != EventKind::ServiceRequest {
        return Err(unsupported("disable_event"));
    }
    if session.srq_state().enabled {
        session.srq_state().enabled = false;
        enable_srq(session, false)?;
    }
    if let Some(channel) = &session.srq_state().channel {
        *channel
            .pending
            .count
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = 0;
    }
    Ok(())
}

/// Tears the channel down as the session closes. Failures are logged.
pub(crate) fn close(session: &mut Vxi11Session) {
    if session.srq_state().channel.is_none() {
        return;
    }
    if session.srq_state().enabled
        && let Err(e) = enable_srq(session, false)
    {
        tracing::debug!(error = %e, "vxi11: disabling SRQ failed");
    }
    let result = session
        .call(DESTROY_INTR_CHAN, &[], Some(SETUP_WAIT))
        .and_then(|reply| check(proto::decode_error(&reply)?, "destroy_intr_chan"));
    if let Err(e) = result {
        tracing::debug!(error = %e, "vxi11: destroy_intr_chan failed");
    }
    session.srq_state().channel = None;
}

/// The server thread: accepts the device's connections and counts SRQs
/// carrying `handle`.
fn serve(listener: &TcpListener, pending: &Pending, stop: &AtomicBool, handle: &[u8]) {
    let mut conns: Vec<(TcpStream, RecordReader)> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let mut busy = false;
        match listener.accept() {
            Ok((stream, peer)) => {
                tracing::debug!(%peer, "vxi11: interrupt channel connected");
                if stream.set_nonblocking(true).is_ok()
                    && stream.set_write_timeout(Some(SETUP_WAIT)).is_ok()
                {
                    conns.push((stream, RecordReader::default()));
                }
                busy = true;
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(e) => {
                tracing::debug!(error = %e, "vxi11: interrupt channel accept failed");
                return;
            }
        }
        conns.retain_mut(|(stream, reader)| match reader.next(stream, 1024) {
            Ok(Some(record)) => {
                busy = true;
                on_call(stream, &record, pending, handle)
            }
            Ok(None) => true,
            Err(e) => {
                tracing::debug!(error = %e, "vxi11: interrupt channel closed");
                false
            }
        });
        if !busy {
            thread::sleep(POLL);
        }
    }
}

/// Handles one call on the interrupt channel. Returns whether to keep the
/// connection.
fn on_call(stream: &mut TcpStream, record: &[u8], pending: &Pending, handle: &[u8]) -> bool {
    let Ok(call) = decode_call(record) else {
        tracing::debug!("vxi11: malformed call on the interrupt channel");
        return false;
    };
    let stat = if call.prog != DEVICE_INTR {
        AcceptStat::ProgUnavail
    } else if call.proc != DEVICE_INTR_SRQ {
        AcceptStat::ProcUnavail
    } else {
        match proto::decode_srq_handle(call.args) {
            Ok(got) if got == handle => {
                tracing::debug!("vxi11: service request");
                *pending.count.lock().unwrap_or_else(PoisonError::into_inner) += 1;
                pending.arrived.notify_all();
                AcceptStat::Success
            }
            Ok(got) => {
                tracing::debug!(?got, "vxi11: SRQ with another handle ignored");
                AcceptStat::Success
            }
            Err(_) => AcceptStat::GarbageArgs,
        }
    };
    // Devices treat the SRQ call as one-way; the reply is a courtesy for
    // those that wait for one.
    let _ = stream.set_nonblocking(false);
    let sent = send(stream, &frame(&encode_reply(call.xid, stat, &[])));
    let _ = stream.set_nonblocking(true);
    sent.is_ok()
}
