//! Phase 2 exit tests (DESIGN.md S8). All are `#[ignore]`; run them with
//! `-- --ignored`.
//!
//! # Against an instrument
//!
//! ```text
//! RVISA_HW_SOCKET=TCPIP0::192.168.1.50::5025::SOCKET \
//! RVISA_HW_BLOCK_CMD="<query returning a binary block>" \
//! RVISA_HW_LARGE_CMD="<query returning >= 1 MiB block>" \
//! RVISA_HW_GOLDEN_DIR=traces/<instrument>/socket \
//!   cargo test -p rvisa-socket --test hardware -- --ignored --nocapture --test-threads=1
//! ```
//!
//! | Variable | Meaning |
//! |---|---|
//! | `RVISA_HW_SOCKET` | resource to test (required) |
//! | `RVISA_HW_IDN_PREFIX`, `RVISA_HW_MULTI_CMD`, `RVISA_HW_BLOCK_CMD`, `RVISA_HW_BLOCK_LEN`, `RVISA_HW_LARGE_CMD` | as for the vendor hardware tests |
//! | `RVISA_HW_ALLOW` | further tests allowed to be unsupported or skipped |
//! | `RVISA_HW_TRACE_DIR` | if set, each test's session is recorded to `<dir>/<test>.jsonl` |
//! | `RVISA_HW_GOLDEN_DIR` | the vendor traces to compare with (`socket_matches_golden_traces`) |
//! | `RVISA_HW_VENDOR` | resource for the vendor side of `throughput_1mb` (default: `RVISA_HW_SOCKET`) |
//!
//! Commands are given without a terminator; `\n` is appended. `--test-threads=1`
//! keeps the tests from talking to the instrument at the same time.
//!
//! # Without an instrument (needs a VISA)
//!
//! The `*_on_simulator` tests run the same checks against the mock
//! `TcpInstrument`, with vendor VISA as the reference.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{SOCKET_ALLOWED, compare, demo_profile, run_recorded};
use rvisa_conformance::{Profile, TESTS};
use rvisa_core::{Backend, OpenOptions, ResourceName, Session, SessionExt};
use rvisa_mock::{RecordingSession, TcpInstrument, Trace, demo_script};
use rvisa_socket::SocketBackend;
use rvisa_vendor::VendorBackend;

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn command(name: &str) -> Option<Vec<u8>> {
    var(name).map(|c| format!("{c}\n").into_bytes())
}

fn resource(name: &str) -> ResourceName {
    var(name)
        .unwrap_or_else(|| panic!("set {name}"))
        .parse()
        .unwrap()
}

/// The profile from the environment. Sockets always need the term char.
fn hardware_profile() -> Profile {
    let mut profile = Profile::scpi().term_char_always(true);
    if let Some(prefix) = var("RVISA_HW_IDN_PREFIX") {
        profile = profile.idn_prefix(prefix);
    }
    if let Some(cmd) = command("RVISA_HW_MULTI_CMD") {
        profile = profile.multi_line_command(cmd);
    }
    if let Some(cmd) = command("RVISA_HW_BLOCK_CMD") {
        let len = var("RVISA_HW_BLOCK_LEN").map(|l| l.parse().expect("RVISA_HW_BLOCK_LEN"));
        profile = profile.binary_block_command(cmd, len);
    }
    if let Some(cmd) = command("RVISA_HW_LARGE_CMD") {
        profile = profile.large_block_command(cmd);
    }
    profile
}

fn allowed() -> Vec<String> {
    let mut allowed: Vec<String> = SOCKET_ALLOWED.iter().map(|s| (*s).to_owned()).collect();
    allowed.extend(
        var("RVISA_HW_ALLOW")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
    );
    allowed
}

fn named(traces: Vec<Trace>) -> Vec<(&'static str, Trace)> {
    TESTS.iter().map(|(name, _)| *name).zip(traces).collect()
}

#[test]
#[ignore = "needs an instrument (RVISA_HW_SOCKET)"]
fn socket_conformance() {
    let rn = resource("RVISA_HW_SOCKET");
    let backend = SocketBackend::new();
    let (report, traces) = run_recorded(
        || backend.open(&rn, &OpenOptions::new()),
        "socket",
        &hardware_profile(),
    );
    println!("{rn}:\n{report}");
    if let Some(dir) = var("RVISA_HW_TRACE_DIR").map(PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        for (name, trace) in named(traces) {
            trace.save(dir.join(format!("{name}.jsonl"))).unwrap();
        }
        println!("traces saved to {}", dir.display());
    }
    let allowed = allowed();
    report.assert_conformant(&allowed.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
#[ignore = "needs an instrument (RVISA_HW_SOCKET) and vendor traces (RVISA_HW_GOLDEN_DIR)"]
fn socket_matches_golden_traces() {
    let rn = resource("RVISA_HW_SOCKET");
    let dir = PathBuf::from(var("RVISA_HW_GOLDEN_DIR").expect("set RVISA_HW_GOLDEN_DIR"));
    let golden: Vec<(&str, Trace)> = TESTS
        .iter()
        .filter_map(|(name, _)| {
            let path = dir.join(format!("{name}.jsonl"));
            path.exists().then(|| (*name, Trace::load(&path).unwrap()))
        })
        .collect();
    assert!(!golden.is_empty(), "no traces in {}", dir.display());
    println!(
        "comparing {} golden traces from {}",
        golden.len(),
        dir.display()
    );

    let backend = SocketBackend::new();
    let (report, traces) = run_recorded(
        || backend.open(&rn, &OpenOptions::new()),
        "socket",
        &hardware_profile(),
    );
    println!("{rn}:\n{report}");
    let diffs = compare(&golden, &named(traces));
    assert!(
        diffs.is_empty(),
        "differences from the vendor traces:\n{}",
        diffs.join("\n")
    );
}

/// Times `runs` reads of the 1 MiB block on a session; returns MB/s
/// (best run) and the block size.
fn throughput(session: &mut dyn Session, command: &[u8], runs: usize) -> (f64, usize) {
    session.set_timeout(Some(Duration::from_secs(30))).unwrap();
    session.set_read_termination(Some(b'\n')).unwrap();
    let mut best = Duration::MAX;
    let mut len = 0;
    for _ in 0..runs {
        let start = Instant::now();
        session.write_all(command).unwrap();
        len = session.read_ieee488_block().unwrap().len();
        best = best.min(start.elapsed());
    }
    #[allow(clippy::cast_precision_loss)]
    let mbps = len as f64 / best.as_secs_f64() / 1e6;
    (mbps, len)
}

fn compare_throughput(socket_rn: &ResourceName, vendor_rn: &ResourceName, command: &[u8]) {
    const RUNS: usize = 5;
    let vendor = VendorBackend::new();
    let mut v = vendor.open(vendor_rn, &OpenOptions::new()).unwrap();
    let (vendor_mbps, len) = throughput(v.as_mut(), command, RUNS);
    drop(v);
    let socket = SocketBackend::new();
    let mut s = socket.open(socket_rn, &OpenOptions::new()).unwrap();
    let (socket_mbps, _) = throughput(s.as_mut(), command, RUNS);
    let ratio = socket_mbps / vendor_mbps;
    println!(
        "{len} bytes, best of {RUNS}: vendor {vendor_mbps:.1} MB/s, socket {socket_mbps:.1} MB/s ({:.0}% of vendor)",
        ratio * 100.0
    );
    // ROADMAP Phase 2: within 10% of the vendor backend.
    assert!(ratio >= 0.9, "socket is more than 10% slower than vendor");
}

#[test]
#[ignore = "needs an instrument (RVISA_HW_SOCKET, RVISA_HW_LARGE_CMD) and a VISA"]
fn throughput_1mb() {
    let socket_rn = resource("RVISA_HW_SOCKET");
    let vendor_rn =
        var("RVISA_HW_VENDOR").map_or_else(|| socket_rn.clone(), |v| v.parse().unwrap());
    let command = command("RVISA_HW_LARGE_CMD").expect("set RVISA_HW_LARGE_CMD");
    compare_throughput(&socket_rn, &vendor_rn, &command);
}

#[test]
#[ignore = "needs a VISA installed"]
fn socket_matches_vendor_on_simulator() {
    let server = TcpInstrument::spawn(demo_script("SIM100", "SIM001")).unwrap();
    let vendor = VendorBackend::new();
    println!("VISA: {}", vendor.library_path().unwrap());
    let (vendor_report, vendor_traces) = run_recorded(
        || vendor.open(server.resource(), &OpenOptions::new()),
        "vendor",
        &demo_profile(),
    );
    let socket = SocketBackend::new();
    let (socket_report, socket_traces) = run_recorded(
        || socket.open(server.resource(), &OpenOptions::new()),
        "socket",
        &demo_profile(),
    );
    println!("vendor:\n{vendor_report}\nsocket:\n{socket_report}");
    socket_report.assert_conformant(SOCKET_ALLOWED);
    let diffs = compare(&named(vendor_traces), &named(socket_traces));
    assert!(
        diffs.is_empty(),
        "differences from vendor:\n{}",
        diffs.join("\n")
    );

    // A trace saved to disk and loaded back compares the same way, as the
    // hardware test does with committed traces.
    let mut file = Vec::new();
    let rn = server.resource();
    let session = vendor.open(rn, &OpenOptions::new()).unwrap();
    let mut rec = RecordingSession::new(session, "vendor", &mut file).unwrap();
    rec.set_read_termination(Some(b'\n')).unwrap();
    rec.query_bytes(b"*IDN?\n").unwrap();
    drop(rec.finish().unwrap());
    let loaded = Trace::read_jsonl(file.as_slice()).unwrap();
    assert_eq!(loaded.responses().len(), 1);
}

#[test]
#[ignore = "needs a VISA installed"]
fn throughput_on_simulator() {
    let server = TcpInstrument::spawn(demo_script("SIM100", "SIM001")).unwrap();
    compare_throughput(server.resource(), server.resource(), b"BLOCK:LARGE?\n");
}
