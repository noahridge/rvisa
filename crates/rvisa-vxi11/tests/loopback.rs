//! The VXI-11 backend over loopback, against `Vxi11Server`: conformance
//! with the mock instrument, and the protocol details (DESIGN.md R4–R9).
//! These run in CI.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::{Sim, demo_profile};
use rvisa_conformance::run_all;
use rvisa_core::{
    Attr, AttrValue, Backend, Error, Event, EventKind, InterfaceType, LockKind, OpenOptions,
    ReadResult, ResourceName, Session, SessionExt, TermReason,
};
use rvisa_vxi11::{Vxi11Backend, Vxi11Server, Vxi11Session};

const SHORT: Duration = Duration::from_millis(200);

fn open(sim: &Sim) -> Vxi11Session {
    sim.backend()
        .open_vxi11(sim.resource(), &OpenOptions::new())
        .unwrap()
}

#[test]
fn conformance_against_the_mock_instrument() {
    let sim = Sim::new();
    let backend = sim.backend();
    let report = run_all(
        || backend.open(sim.resource(), &OpenOptions::new()),
        &demo_profile(),
    );
    println!("{report}");
    report.assert_all_passed();
}

#[test]
fn reads_end_at_end_term_char_or_count() {
    let sim = Sim::new();
    let mut s = open(&sim);
    sim.instrument.push_response(b"line1\nline2\n");
    let r = s.read(3).unwrap();
    assert_eq!((r.data, r.reason), (b"lin".to_vec(), TermReason::MaxCount));
    s.set_read_termination(Some(b'\n')).unwrap();
    let r = s.read(100).unwrap();
    assert_eq!((r.data, r.reason), (b"e1\n".to_vec(), TermReason::TermChar));
    // END wins over the term char on the message's last byte.
    let r = s.read(100).unwrap();
    assert_eq!((r.data, r.reason), (b"line2\n".to_vec(), TermReason::End));
    assert!(matches!(s.read(0), Err(Error::InvalidArgument(_))));
}

#[test]
fn a_read_with_nothing_to_receive_times_out() {
    let sim = Sim::new();
    let mut s = open(&sim);
    s.set_timeout(Some(SHORT)).unwrap();
    let start = Instant::now();
    match s.read(10) {
        Err(Error::Timeout { partial, .. }) => assert!(partial.is_empty()),
        other => panic!("{other:?}"),
    }
    assert!(start.elapsed() >= SHORT * 3 / 4);
    assert_eq!(sim.server.aborts(), 0, "the device answered in time");
}

#[test]
fn writes_are_split_by_max_recv_size_with_end_on_the_last_part() {
    let sim = Sim::new();
    let server = sim.server.max_recv_size(4);
    let sim = Sim {
        server,
        instrument: sim.instrument,
    };
    let mut s = open(&sim);
    assert_eq!(s.max_recv_size(), 4);
    s.write_all(b"*RST;*CLS\n").unwrap();
    assert_eq!(sim.server.write_sizes(), [4, 4, 2]);
    // The instrument got one message.
    assert_eq!(sim.instrument.writes(), [b"*RST;*CLS\n".to_vec()]);
    assert_eq!(
        s.query_bytes(b"*IDN?\n").unwrap(),
        b"ACME Instruments,SIM100,SIM001,1.0.0\n"
    );
}

#[test]
fn without_send_end_a_message_continues() {
    let sim = Sim::new();
    let mut s = open(&sim);
    s.set_attr(Attr::SendEndEnabled, AttrValue::Bool(false))
        .unwrap();
    s.write_all(b"*IDN").unwrap();
    assert!(sim.instrument.writes().is_empty(), "no END yet");
    s.set_attr(Attr::SendEndEnabled, AttrValue::Bool(true))
        .unwrap();
    s.write_all(b"?\n").unwrap();
    assert_eq!(sim.instrument.writes(), [b"*IDN?\n".to_vec()]);
}

#[test]
fn status_byte_trigger_and_clear_are_native() {
    let sim = Sim::new();
    let mut s = open(&sim);
    sim.instrument.raise_srq(0x10);
    assert_eq!(s.read_stb().unwrap(), 0x50);
    assert_eq!(s.read_stb().unwrap(), 0x10, "the poll cleared RQS");
    s.assert_trigger().unwrap();
    assert_eq!(sim.instrument.triggers(), 1);
    s.clear().unwrap();
    assert_eq!(sim.instrument.clears(), 1);
}

#[test]
fn service_requests_arrive_on_the_interrupt_channel() {
    let sim = Sim::new();
    let mut s = open(&sim);
    assert!(matches!(
        s.wait_event(EventKind::ServiceRequest, SHORT),
        Err(Error::EventNotEnabled(EventKind::ServiceRequest))
    ));
    s.enable_event(EventKind::ServiceRequest).unwrap();
    assert!(
        s.wait_event(EventKind::ServiceRequest, SHORT)
            .unwrap_err()
            .is_timeout()
    );
    sim.instrument.raise_srq(0x10);
    sim.instrument.raise_srq(0x10);
    for _ in 0..2 {
        let event = s
            .wait_event(EventKind::ServiceRequest, Duration::from_secs(5))
            .unwrap();
        assert_eq!(event.kind, EventKind::ServiceRequest);
    }
    s.disable_event(EventKind::ServiceRequest).unwrap();
    assert!(matches!(
        s.wait_event(EventKind::ServiceRequest, SHORT),
        Err(Error::EventNotEnabled(_))
    ));
    assert!(matches!(
        s.enable_event(EventKind::Trigger),
        Err(Error::Unsupported { .. })
    ));
    // Re-enabling reuses the channel.
    s.enable_event(EventKind::ServiceRequest).unwrap();
    sim.instrument.raise_srq(0x10);
    s.wait_event(EventKind::ServiceRequest, Duration::from_secs(5))
        .unwrap();
}

#[test]
fn locks_shut_out_other_sessions_and_other_clients() {
    let sim = Sim::new();
    let backend = sim.backend();
    let mut a = backend
        .open_vxi11(sim.resource(), &OpenOptions::new())
        .unwrap();
    let mut b = backend
        .open_vxi11(sim.resource(), &OpenOptions::new())
        .unwrap();
    // Another backend stands in for another process.
    let mut other = open(&sim);

    a.lock(LockKind::Exclusive, SHORT).unwrap();
    a.lock(LockKind::Exclusive, SHORT).unwrap();
    assert!(matches!(b.write(b"*RST\n"), Err(Error::ResourceLocked)));
    assert!(
        matches!(other.write(b"*RST\n"), Err(Error::ResourceLocked)),
        "device lock"
    );
    assert!(matches!(
        other.lock(LockKind::Exclusive, SHORT),
        Err(Error::ResourceLocked)
    ));
    a.unlock().unwrap();
    assert!(
        matches!(other.write(b"*RST\n"), Err(Error::ResourceLocked)),
        "still nested"
    );
    a.unlock().unwrap();
    other.write_all(b"*RST\n").unwrap();
    b.write_all(b"*RST\n").unwrap();
    assert!(matches!(a.unlock(), Err(Error::InvalidArgument(_))));

    // Shared locks stay in process.
    let shared = LockKind::Shared { key: "k".into() };
    a.lock(shared.clone(), SHORT).unwrap();
    b.lock(shared, SHORT).unwrap();
    a.write_all(b"*RST\n").unwrap();
    other.write_all(b"*RST\n").unwrap();
    drop(a);
    drop(b);

    // A lock asked for at open; closing the session releases it.
    let locked = backend
        .open_vxi11(
            sim.resource(),
            &OpenOptions::new()
                .lock(LockKind::Exclusive)
                .open_timeout(SHORT),
        )
        .unwrap();
    assert!(matches!(other.write(b"*RST\n"), Err(Error::ResourceLocked)));
    drop(locked);
    other.write_all(b"*RST\n").unwrap();
}

#[test]
fn attributes() {
    let sim = Sim::new();
    let mut s = open(&sim);
    assert_eq!(
        s.get_attr(Attr::InterfaceType).unwrap(),
        AttrValue::InterfaceType(InterfaceType::Tcpip)
    );
    assert_eq!(
        s.get_attr(Attr::TimeoutValue).unwrap(),
        AttrValue::Timeout(Some(Duration::from_secs(2)))
    );
    assert_eq!(
        s.get_attr(Attr::TcpipNodelay).unwrap(),
        AttrValue::Bool(true)
    );
    s.set_attr(Attr::TcpipKeepalive, AttrValue::Bool(true))
        .unwrap();
    assert_eq!(
        s.get_attr(Attr::TcpipKeepalive).unwrap(),
        AttrValue::Bool(true)
    );
    assert!(s.link_id() > 0);
    assert_eq!(s.peer_addr().unwrap(), sim.server.addr());
}

#[test]
fn unknown_devices_and_hosts_are_not_found() {
    let sim = Sim::new();
    let rn: ResourceName = "TCPIP0::127.0.0.1::gpib0,5::INSTR".parse().unwrap();
    assert!(matches!(
        sim.backend().open(&rn, &OpenOptions::new()),
        Err(Error::NotFound(_))
    ));

    // Nothing listens on the portmapper port.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let backend = Vxi11Backend::new().portmapper_port(port);
    assert!(matches!(
        backend.open(sim.resource(), &OpenOptions::new()),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn discovery_finds_the_server() {
    let sim = Sim::new();
    let target = std::net::SocketAddr::from(([127, 0, 0, 1], sim.server.port()));
    let backend = sim
        .backend()
        .discovery_targets([target])
        .discovery(Some(SHORT));
    let expected: ResourceName = "TCPIP0::127.0.0.1::inst0::INSTR".parse().unwrap();
    assert_eq!(
        backend.discover(SHORT).unwrap(),
        std::slice::from_ref(&expected)
    );
    assert_eq!(backend.list("TCPIP?*").unwrap(), [expected]);
    assert!(backend.list("USB?*").unwrap().is_empty());
}

#[test]
fn the_abort_handle_reaches_the_abort_channel() {
    let sim = Sim::new();
    let s = open(&sim);
    let handle = s.abort_handle();
    handle.abort().unwrap();
    thread::spawn(move || handle.abort().unwrap())
        .join()
        .unwrap();
    assert_eq!(sim.server.aborts(), 2);
}

/// A delay, then what a read returns.
type ScriptedRead = (Duration, rvisa_core::Result<ReadResult>);

/// A session that answers reads in a scripted way, to reach cases the mock
/// cannot: partial data before a timeout, and a device that hangs.
#[derive(Clone)]
struct Scripted {
    reads: Arc<Mutex<Vec<ScriptedRead>>>,
    rn: ResourceName,
}

impl Scripted {
    fn new(reads: Vec<ScriptedRead>) -> Self {
        Self {
            reads: Arc::new(Mutex::new(reads)),
            rn: "TCPIP0::scripted::INSTR".parse().unwrap(),
        }
    }
}

fn unsupported() -> Error {
    Error::Unsupported {
        backend: "scripted",
        op: "op",
    }
}

impl Session for Scripted {
    fn resource(&self) -> &ResourceName {
        &self.rn
    }
    fn write(&mut self, data: &[u8]) -> rvisa_core::Result<usize> {
        Ok(data.len())
    }
    fn read(&mut self, _max: usize) -> rvisa_core::Result<ReadResult> {
        let next = self.reads.lock().unwrap().remove(0);
        thread::sleep(next.0);
        next.1
    }
    fn set_timeout(&mut self, _: Option<Duration>) -> rvisa_core::Result<()> {
        Ok(())
    }
    fn timeout(&self) -> Option<Duration> {
        None
    }
    fn set_read_termination(&mut self, _: Option<u8>) -> rvisa_core::Result<()> {
        Ok(())
    }
    fn clear(&mut self) -> rvisa_core::Result<()> {
        Ok(())
    }
    fn read_stb(&mut self) -> rvisa_core::Result<u8> {
        Err(unsupported())
    }
    fn assert_trigger(&mut self) -> rvisa_core::Result<()> {
        Err(unsupported())
    }
    fn lock(&mut self, _: LockKind, _: Duration) -> rvisa_core::Result<()> {
        Err(unsupported())
    }
    fn unlock(&mut self) -> rvisa_core::Result<()> {
        Err(unsupported())
    }
    fn get_attr(&self, _: Attr) -> rvisa_core::Result<AttrValue> {
        Err(unsupported())
    }
    fn set_attr(&mut self, _: Attr, _: AttrValue) -> rvisa_core::Result<()> {
        Err(unsupported())
    }
    fn enable_event(&mut self, _: EventKind) -> rvisa_core::Result<()> {
        Err(unsupported())
    }
    fn wait_event(&mut self, _: EventKind, _: Duration) -> rvisa_core::Result<Event> {
        Err(unsupported())
    }
    fn disable_event(&mut self, _: EventKind) -> rvisa_core::Result<()> {
        Err(unsupported())
    }
}

fn scripted_server(script: &Scripted) -> (Vxi11Server, Vxi11Backend) {
    let script = script.clone();
    let server =
        Vxi11Server::spawn(move |_| Ok(Box::new(script.clone()) as Box<dyn Session>)).unwrap();
    let backend = Vxi11Backend::new().portmapper_port(server.port());
    (server, backend)
}

fn ok(data: &[u8], reason: TermReason) -> rvisa_core::Result<ReadResult> {
    Ok(ReadResult {
        data: data.to_vec(),
        reason,
    })
}

#[test]
fn a_timed_out_read_keeps_its_bytes() {
    let script = Scripted::new(vec![
        (Duration::ZERO, Err(Error::read_timeout(b"PART".to_vec()))),
        (Duration::ZERO, ok(b"IAL\n", TermReason::End)),
    ]);
    let (server, backend) = scripted_server(&script);
    let mut s = backend
        .open(server.resource(), &OpenOptions::new())
        .unwrap();
    match s.read_message() {
        Err(Error::Timeout { partial, .. }) => assert_eq!(partial, b"PART"),
        other => panic!("{other:?}"),
    }
    assert_eq!(s.read_message().unwrap(), b"IAL\n");
}

#[test]
fn reads_without_a_reason_continue() {
    // A device that returns part of the request with no reason bits, as
    // some do at their buffer size: the client asks for the rest.
    let script = Scripted::new(vec![
        (Duration::ZERO, ok(b"AB", TermReason::MaxCount)),
        (Duration::ZERO, ok(b"CD", TermReason::End)),
    ]);
    let (server, backend) = scripted_server(&script);
    let mut s = backend
        .open(server.resource(), &OpenOptions::new())
        .unwrap();
    // The server reports MaxCount as REQCNT, which with fewer bytes than
    // asked is not an end: the read goes on.
    let r = s.read(100).unwrap();
    assert_eq!((r.data, r.reason), (b"ABCD".to_vec(), TermReason::End));
}

#[test]
fn a_hung_device_is_aborted_and_its_late_reply_ignored() {
    let script = Scripted::new(vec![
        // Ignores the 100 ms I/O timeout; the client gives up after it
        // plus a second of slack.
        (Duration::from_millis(1800), ok(b"late\n", TermReason::End)),
        (Duration::ZERO, ok(b"fresh\n", TermReason::End)),
    ]);
    let (server, backend) = scripted_server(&script);
    let mut s = backend
        .open(server.resource(), &OpenOptions::new())
        .unwrap();
    s.set_timeout(Some(Duration::from_millis(100))).unwrap();
    let start = Instant::now();
    assert!(s.read(100).unwrap_err().is_timeout());
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(1000) && elapsed < Duration::from_millis(1800),
        "{elapsed:?}"
    );
    assert_eq!(server.aborts(), 1);
    // The next call skips the stale reply.
    s.set_timeout(Some(Duration::from_secs(5))).unwrap();
    assert_eq!(s.read(100).unwrap().data, b"fresh\n");
}
