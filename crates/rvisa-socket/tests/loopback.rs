//! The socket backend over loopback: conformance against the mock TCP
//! instrument, and NI-VISA's observed behaviour (DESIGN.md S1–S6) against a
//! raw peer. These run in CI.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::thread;
use std::time::{Duration, Instant};

use common::{Peer, SOCKET_ALLOWED, demo_profile};
use rvisa_conformance::run_all;
use rvisa_core::{
    Attr, AttrValue, Backend, Error, EventKind, InterfaceType, LockKind, OpenOptions, Session,
    SessionExt, TermReason,
};
use rvisa_mock::{TcpInstrument, demo_script};
use rvisa_socket::{ATTR_IO_PROT, SocketBackend, SocketSession};

const SHORT: Duration = Duration::from_millis(200);

fn open(peer: &Peer, backend: &SocketBackend) -> SocketSession {
    backend
        .open_socket(&peer.resource, &OpenOptions::new())
        .unwrap()
}

fn timeout_partial(result: rvisa_core::Result<impl std::fmt::Debug>) -> Vec<u8> {
    match result {
        Err(Error::Timeout { partial, .. }) => partial,
        other => panic!("expected a timeout, got {other:?}"),
    }
}

#[test]
fn conformance_against_the_mock_tcp_instrument() {
    let server = TcpInstrument::spawn(demo_script("SIM100", "SIM001")).unwrap();
    let backend = SocketBackend::new();
    let report = run_all(
        || backend.open(server.resource(), &OpenOptions::new()),
        &demo_profile(),
    );
    println!("{report}");
    report.assert_conformant(SOCKET_ALLOWED);
}

#[test]
fn a_timed_out_read_keeps_its_bytes() {
    // NI-VISA probe case A: the term char is on, the reply arrives in two
    // parts with a pause longer than the timeout.
    let peer = Peer::new();
    let mut s = open(&peer, &SocketBackend::new());
    let mut conn = peer.accept();
    s.set_timeout(Some(SHORT)).unwrap();
    s.set_read_termination(Some(b'\n')).unwrap();
    conn.send(b"PARTIAL");
    let start = Instant::now();
    assert_eq!(timeout_partial(s.read(100)), b"PARTIAL");
    assert!(start.elapsed() >= SHORT * 3 / 4);
    conn.send(b"\n");
    let rest = s.read(100).unwrap();
    assert_eq!(
        (rest.data, rest.reason),
        (b"\n".to_vec(), TermReason::TermChar)
    );
}

#[test]
fn without_a_term_char_only_the_count_ends_a_read() {
    // NI-VISA probe case B.
    let peer = Peer::new();
    let mut s = open(&peer, &SocketBackend::new());
    let mut conn = peer.accept();
    s.set_timeout(Some(SHORT)).unwrap();
    conn.send(b"PARTIAL");
    assert_eq!(timeout_partial(s.read(100)), b"PARTIAL");
    conn.send(b"REST\n");
    assert_eq!(timeout_partial(s.read(100)), b"REST\n");
    assert!(timeout_partial(s.read(100)).is_empty());
    conn.send(b"0123456789");
    let head = s.read(4).unwrap();
    assert_eq!(
        (head.data, head.reason),
        (b"0123".to_vec(), TermReason::MaxCount)
    );
}

#[test]
fn helpers_keep_the_whole_message_on_timeout() {
    let peer = Peer::new();
    let mut s = open(&peer, &SocketBackend::new());
    let mut conn = peer.accept();
    s.set_timeout(Some(SHORT)).unwrap();
    s.set_read_termination(Some(b'\n')).unwrap();
    // A definite block whose data never completes: everything read so far,
    // header included, comes back.
    conn.send(b"#15ab");
    assert_eq!(timeout_partial(s.read_ieee488_block()), b"#15ab");
}

#[test]
fn reads_split_at_the_term_char_and_count() {
    let peer = Peer::new();
    let mut s = open(&peer, &SocketBackend::new());
    let mut conn = peer.accept();
    s.set_read_termination(Some(b'\n')).unwrap();
    // "ONE" then "TWO\n" within the timeout: one read (probe case C).
    conn.send(b"ONE");
    let sender = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        conn.send(b"TWO\n0123456789ABCDEF\n");
        conn
    });
    let r = s.read(100).unwrap();
    assert_eq!(
        (r.data, r.reason),
        (b"ONETWO\n".to_vec(), TermReason::TermChar)
    );
    let _conn = sender.join().unwrap();
    let r = s.read(5).unwrap();
    assert_eq!(
        (r.data, r.reason),
        (b"01234".to_vec(), TermReason::MaxCount)
    );
    assert_eq!(s.read_message().unwrap(), b"56789ABCDEF\n");
}

#[test]
fn writes_are_sent_unchanged() {
    let peer = Peer::new();
    let mut s = open(&peer, &SocketBackend::new());
    let mut conn = peer.accept();
    s.set_read_termination(Some(b'\n')).unwrap();
    s.set_attr(Attr::SendEndEnabled, AttrValue::Bool(false))
        .unwrap();
    assert_eq!(s.write(b"ABC").unwrap(), 3);
    assert_eq!(conn.expect(3), b"ABC");
    assert!(conn.received().is_empty(), "nothing appended");
}

#[test]
fn clear_discards_received_input_and_sends_the_clear_command() {
    let peer = Peer::new();
    let mut s = open(&peer, &SocketBackend::new().clear_command(b"ABORT\n"));
    let mut conn = peer.accept();
    s.set_read_termination(Some(b'\n')).unwrap();
    conn.send(b"0123456789\nstale\n");
    assert_eq!(s.read(4).unwrap().data, b"0123");
    thread::sleep(Duration::from_millis(50)); // everything has arrived
    s.clear().unwrap();
    assert_eq!(conn.expect(6), b"ABORT\n");
    conn.send(b"fresh\n");
    assert_eq!(s.read_message().unwrap(), b"fresh\n");
}

#[test]
fn status_byte_and_trigger_need_488_strings() {
    let peer = Peer::new();
    let mut s = open(&peer, &SocketBackend::new());
    let mut conn = peer.accept();
    assert!(matches!(
        s.read_stb(),
        Err(Error::Unsupported { op: "read_stb", .. })
    ));
    assert!(matches!(
        s.assert_trigger(),
        Err(Error::Unsupported {
            op: "assert_trigger",
            ..
        })
    ));
    assert!(conn.received().is_empty(), "nothing sent (as NI-VISA)");

    // Per session, the way vendor-backend users set VI_ATTR_IO_PROT. The
    // reply is read up to `\n` even with the term char off (S5).
    s.set_attr(Attr::Raw(ATTR_IO_PROT), AttrValue::U64(4))
        .unwrap();
    conn.send(b"+72\r\n");
    assert_eq!(s.read_stb().unwrap(), 72);
    assert_eq!(conn.expect(6), b"*STB?\n");
    s.assert_trigger().unwrap();
    assert_eq!(conn.expect(5), b"*TRG\n");
    conn.send(b"oops\n");
    assert!(matches!(s.read_stb(), Err(Error::InvalidData(_))));

    // For every session of a backend.
    let peer = Peer::new();
    let mut s = open(&peer, &SocketBackend::new().emulate_488_strings(true));
    let mut conn = peer.accept();
    s.assert_trigger().unwrap();
    assert_eq!(conn.expect(5), b"*TRG\n");
    assert_eq!(
        s.get_attr(Attr::Raw(ATTR_IO_PROT)).unwrap(),
        AttrValue::U64(4)
    );
}

#[test]
fn a_closed_connection_is_an_io_error() {
    let peer = Peer::new();
    let mut s = open(&peer, &SocketBackend::new());
    drop(peer.accept());
    match s.read(10) {
        Err(Error::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
        other => panic!("{other:?}"),
    }
}

#[test]
fn locks_shut_out_other_sessions_of_the_backend() {
    let peer = Peer::new();
    let backend = SocketBackend::new();
    let mut a = open(&peer, &backend);
    let _ca = peer.accept();
    let mut b = open(&peer, &backend);
    let _cb = peer.accept();

    a.lock(LockKind::Exclusive, SHORT).unwrap();
    assert!(matches!(b.write(b"x"), Err(Error::ResourceLocked)));
    assert!(matches!(b.read(1), Err(Error::ResourceLocked)));
    assert!(matches!(
        b.lock(LockKind::Exclusive, Duration::from_millis(20)),
        Err(Error::ResourceLocked)
    ));
    a.unlock().unwrap();
    assert!(a.unlock().is_err(), "a second unlock fails, as in NI-VISA");
    b.lock(LockKind::Shared { key: "k".into() }, SHORT).unwrap();
    a.lock(LockKind::Shared { key: "k".into() }, SHORT).unwrap();
    assert_eq!(a.write(b"ok").unwrap(), 2);
    drop(b); // releases its share
    a.unlock().unwrap();

    // A lock requested when opening.
    let c = backend
        .open_socket(
            &peer.resource,
            &OpenOptions::new()
                .lock(LockKind::Exclusive)
                .open_timeout(SHORT),
        )
        .unwrap();
    let _cc = peer.accept();
    assert!(matches!(a.write(b"x"), Err(Error::ResourceLocked)));
    drop(c);
    assert_eq!(a.write(b"x").unwrap(), 1);
}

#[test]
fn attributes() {
    let peer = Peer::new();
    let mut s = open(&peer, &SocketBackend::new().keepalive(false));
    let _conn = peer.accept();
    assert_eq!(
        s.get_attr(Attr::TimeoutValue).unwrap(),
        AttrValue::Timeout(Some(Duration::from_secs(2))),
        "NI-VISA's default"
    );
    assert_eq!(
        s.get_attr(Attr::TermCharEnabled).unwrap(),
        AttrValue::Bool(false)
    );
    assert_eq!(s.get_attr(Attr::TermChar).unwrap(), AttrValue::U8(b'\n'));
    assert_eq!(
        s.get_attr(Attr::InterfaceType).unwrap(),
        AttrValue::InterfaceType(InterfaceType::Tcpip)
    );
    assert_eq!(
        s.get_attr(Attr::TcpipNodelay).unwrap(),
        AttrValue::Bool(true)
    );
    assert_eq!(
        s.get_attr(Attr::TcpipKeepalive).unwrap(),
        AttrValue::Bool(false)
    );
    s.set_attr(Attr::TcpipNodelay, AttrValue::Bool(false))
        .unwrap();
    s.set_attr(Attr::TcpipKeepalive, AttrValue::Bool(true))
        .unwrap();
    assert_eq!(
        s.get_attr(Attr::TcpipNodelay).unwrap(),
        AttrValue::Bool(false)
    );
    assert_eq!(
        s.get_attr(Attr::TcpipKeepalive).unwrap(),
        AttrValue::Bool(true)
    );
    s.set_attr(Attr::TermCharEnabled, AttrValue::Bool(true))
        .unwrap();
    s.set_attr(Attr::TermChar, AttrValue::U8(b'\r')).unwrap();
    assert_eq!(s.get_attr(Attr::TermChar).unwrap(), AttrValue::U8(b'\r'));
    assert!(matches!(
        s.get_attr(Attr::AsrlBaud),
        Err(Error::Unsupported { .. })
    ));
    assert!(matches!(
        s.enable_event(EventKind::ServiceRequest),
        Err(Error::Unsupported { .. })
    ));
}

#[test]
fn opening() {
    let backend = SocketBackend::new();
    let socket = "TCPIP0::127.0.0.1::5025::SOCKET".parse().unwrap();
    let instr = "TCPIP0::127.0.0.1::INSTR".parse().unwrap();
    assert!(backend.supports(&socket));
    assert!(!backend.supports(&instr));
    assert!(matches!(
        backend.open_socket(&instr, &OpenOptions::new()),
        Err(Error::InvalidArgument(_))
    ));
    assert!(backend.list("?*").unwrap().is_empty());
    assert!(backend.list("[").is_err());

    // A port nobody listens on.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let refused = format!("TCPIP0::127.0.0.1::{port}::SOCKET")
        .parse()
        .unwrap();
    assert!(matches!(
        backend.open_socket(&refused, &OpenOptions::new()),
        Err(Error::NotFound(_))
    ));
    let unknown = "TCPIP0::no-such-host.invalid::5025::SOCKET"
        .parse()
        .unwrap();
    assert!(matches!(
        backend.open_socket(&unknown, &OpenOptions::new()),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn ipv6_hosts() {
    let Ok(peer) = Peer::bind("[::1]:0") else {
        eprintln!("no IPv6 loopback here; skipped");
        return;
    };
    assert!(peer.resource.to_string().contains("[::1]"));
    let mut s = open(&peer, &SocketBackend::new());
    let mut conn = peer.accept();
    s.write(b"hi").unwrap();
    assert_eq!(conn.expect(2), b"hi");
}
