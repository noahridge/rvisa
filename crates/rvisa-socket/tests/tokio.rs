//! The native async session (DESIGN.md S7).

#![cfg(feature = "tokio")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{Conn, Peer};
use rvisa_core::{Attr, AttrValue, Error, LockKind, OpenOptions, Session, TermReason};
use rvisa_mock::{TcpInstrument, demo_script};
use rvisa_socket::SocketBackend;
use rvisa_socket::tokio::AsyncSocketSession;

const SHORT: Duration = Duration::from_millis(200);

/// Connects a session to `peer`, accepting on the blocking pool.
async fn connect(backend: &SocketBackend, peer: Peer) -> (AsyncSocketSession, Conn, Peer) {
    let resource = peer.resource.clone();
    let accept = tokio::task::spawn_blocking(move || {
        let conn = peer.accept();
        (conn, peer)
    });
    let session = AsyncSocketSession::connect(backend, &resource)
        .await
        .unwrap();
    let (conn, peer) = accept.await.unwrap();
    (session, conn, peer)
}

#[tokio::test]
async fn queries_blocks_and_clear_against_the_mock_instrument() {
    let server = TcpInstrument::spawn(demo_script("SIM100", "SIM001")).unwrap();
    let backend = SocketBackend::new();
    let mut s = AsyncSocketSession::connect(&backend, server.resource())
        .await
        .unwrap();
    s.set_read_termination(Some(b'\n'));
    assert!(
        s.query("*IDN?")
            .await
            .unwrap()
            .starts_with("ACME Instruments,SIM100")
    );
    assert_eq!(s.query_ieee488_block("DATA?").await.unwrap().len(), 8);
    let large = s.query_ieee488_block("BLOCK:LARGE?").await.unwrap();
    assert!(large.len() >= 1024 * 1024);
    assert!(
        large.windows(1).any(|b| b == b"\n"),
        "data contains the term char"
    );
    // Still in step after the block.
    assert_eq!(s.query("*OPC?").await.unwrap(), "1");

    s.write_str("MULTI?").await.unwrap();
    let first = s.read(64).await.unwrap();
    assert_eq!(
        (first.data, first.reason),
        (b"line1\n".to_vec(), TermReason::TermChar)
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    s.clear().await.unwrap();
    assert_eq!(s.query("*OPC?").await.unwrap(), "1", "line2 was discarded");
}

#[tokio::test]
async fn timeouts_keep_bytes_and_cancelled_reads_lose_nothing() {
    let peer = Peer::new();
    let backend = SocketBackend::new();
    let (mut s, mut conn, _peer) = connect(&backend, peer).await;
    s.set_timeout(Some(SHORT));
    s.set_read_termination(Some(b'\n'));

    conn.send(b"PARTIAL");
    match s.read(100).await {
        Err(Error::Timeout { partial, .. }) => assert_eq!(partial, b"PARTIAL"),
        other => panic!("{other:?}"),
    }

    // A read cancelled after bytes arrived leaves them for the next read.
    conn.send(b"FIR");
    let cancelled = tokio::time::timeout(Duration::from_millis(50), s.read(100)).await;
    assert!(cancelled.is_err());
    conn.send(b"ST\n");
    assert_eq!(s.read_message().await.unwrap(), b"FIRST\n");

    // Writes go out unchanged; query appends the write termination.
    s.write(b"RAW").await.unwrap();
    assert_eq!(conn.expect(3), b"RAW");
    s.set_write_termination(Some(b"\r\n"));
    s.write_str("X").await.unwrap();
    assert_eq!(conn.expect(3), b"X\r\n");
}

#[tokio::test]
async fn emulation_attributes_and_locks() {
    let peer = Peer::new();
    let backend = SocketBackend::new();
    let (mut s, mut conn, peer) = connect(&backend, peer).await;

    assert!(matches!(s.read_stb().await, Err(Error::Unsupported { .. })));
    s.set_attr(Attr::Raw(rvisa_socket::ATTR_IO_PROT), AttrValue::U64(4))
        .unwrap();
    conn.send(b"16\n");
    assert_eq!(s.read_stb().await.unwrap(), 16);
    assert_eq!(conn.expect(6), b"*STB?\n");
    s.assert_trigger().await.unwrap();
    assert_eq!(conn.expect(5), b"*TRG\n");
    assert_eq!(
        s.get_attr(Attr::TcpipNodelay).unwrap(),
        AttrValue::Bool(true)
    );

    // Async and blocking sessions of one backend share locks.
    s.lock(LockKind::Exclusive, SHORT).await.unwrap();
    let res = peer.resource.clone();
    let blocking = backend.clone();
    let accept = tokio::task::spawn_blocking(move || peer.accept());
    let mut other = tokio::task::spawn_blocking(move || {
        blocking.open_socket(&res, &OpenOptions::new()).unwrap()
    })
    .await
    .unwrap();
    let _c2 = accept.await.unwrap();
    assert!(matches!(other.write(b"x"), Err(Error::ResourceLocked)));
    s.unlock().unwrap();
    assert_eq!(other.write(b"x").unwrap(), 1);
    other.lock(LockKind::Exclusive, SHORT).unwrap();
    assert!(matches!(
        s.lock(LockKind::Exclusive, Duration::from_millis(20)).await,
        Err(Error::ResourceLocked)
    ));
}

#[tokio::test]
async fn connect_errors() {
    let backend = SocketBackend::new();
    let instr = "TCPIP0::127.0.0.1::INSTR".parse().unwrap();
    assert!(matches!(
        AsyncSocketSession::connect(&backend, &instr).await,
        Err(Error::InvalidArgument(_))
    ));
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let refused = format!("TCPIP0::127.0.0.1::{port}::SOCKET")
        .parse()
        .unwrap();
    assert!(matches!(
        AsyncSocketSession::connect(&backend, &refused).await,
        Err(Error::NotFound(_))
    ));
}
