//! Helpers shared by the socket backend's tests.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rvisa_conformance::{Profile, Report, TESTS, run_all};
use rvisa_core::{ResourceName, Result, Session};
use rvisa_mock::{RecordingSession, Trace};

/// The far end of a socket session: sees exactly what the session sends,
/// and sends whatever a test wants.
pub struct Peer {
    listener: TcpListener,
    pub resource: ResourceName,
}

impl Peer {
    pub fn new() -> Self {
        Self::bind("127.0.0.1:0").unwrap()
    }

    pub fn bind(addr: &str) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let local = listener.local_addr()?;
        let host = match local {
            SocketAddr::V4(a) => a.ip().to_string(),
            SocketAddr::V6(a) => format!("[{}]", a.ip()),
        };
        let resource = format!("TCPIP0::{host}::{}::SOCKET", local.port())
            .parse()
            .map_err(std::io::Error::other)?;
        Ok(Self { listener, resource })
    }

    /// Accepts the session's connection.
    pub fn accept(&self) -> Conn {
        let (stream, _) = self.listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        stream.set_nodelay(true).unwrap();
        Conn(stream)
    }
}

pub struct Conn(pub TcpStream);

impl Conn {
    pub fn send(&mut self, data: &[u8]) {
        self.0.write_all(data).unwrap();
    }

    /// Reads exactly `n` bytes the session sent.
    pub fn expect(&mut self, n: usize) -> Vec<u8> {
        let mut buf = vec![0; n];
        self.0.read_exact(&mut buf).unwrap();
        buf
    }

    /// Whatever arrives within a short wait.
    pub fn received(&mut self) -> Vec<u8> {
        self.0
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut out = Vec::new();
        let mut buf = [0; 4096];
        while let Ok(n) = self.0.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        out
    }
}

/// A `Write` that several owners can append to.
#[derive(Clone, Default)]
pub struct SharedBuf(pub Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Runs the conformance suite, recording each test's session to a trace.
/// Returns the report and the traces, in suite order.
pub fn run_recorded(
    mut open: impl FnMut() -> Result<Box<dyn Session>>,
    backend: &str,
    profile: &Profile,
) -> (Report, Vec<Trace>) {
    let bufs: Vec<SharedBuf> = TESTS.iter().map(|_| SharedBuf::default()).collect();
    let mut index = 0;
    let report = run_all(
        || {
            let session = open()?;
            let buf = bufs[index].clone();
            index += 1;
            Ok(Box::new(RecordingSession::new(session, backend, buf)?) as Box<dyn Session>)
        },
        profile,
    );
    let traces = bufs
        .iter()
        .map(|b| {
            let bytes = b.0.lock().unwrap().clone();
            Trace::read_jsonl(bytes.as_slice()).unwrap()
        })
        .collect();
    (report, traces)
}

/// Compares two backends' responses test by test (ROADMAP §7.3). Returns
/// a description of each difference.
pub fn compare(golden: &[(&str, Trace)], candidate: &[(&str, Trace)]) -> Vec<String> {
    let mut diffs = Vec::new();
    for (name, want) in golden {
        let Some((_, got)) = candidate.iter().find(|(n, _)| n == name) else {
            diffs.push(format!("{name}: not run"));
            continue;
        };
        let (want, got) = (want.responses(), got.responses());
        if want != got {
            diffs.push(format!(
                "{name}: responses differ\n    golden:    {:?}\n    candidate: {:?}",
                show(&want),
                show(&got)
            ));
        }
    }
    diffs
}

fn show(messages: &[Vec<u8>]) -> Vec<String> {
    messages
        .iter()
        .map(|m| {
            let text = String::from_utf8_lossy(&m[..m.len().min(80)])
                .escape_debug()
                .to_string();
            if m.len() > 80 {
                format!("{text}... ({} bytes)", m.len())
            } else {
                text
            }
        })
        .collect()
}

/// The demo instrument's conformance profile, for a raw socket.
pub fn demo_profile() -> Profile {
    Profile::scpi()
        .term_char_always(true)
        .idn_prefix(b"ACME Instruments,SIM100")
        .multi_line_command(b"MULTI?\n")
        .binary_block_command(b"DATA?\n", Some(8))
        .large_block_command(b"BLOCK:LARGE?\n")
}

/// What a raw socket legitimately cannot do (no SRQ, no events, no
/// trigger without 488.2 string emulation).
pub const SOCKET_ALLOWED: &[&str] = &[
    "stb_and_srq_event",
    "wait_event_requires_enable",
    "assert_trigger",
];
