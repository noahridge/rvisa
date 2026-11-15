//! Phase 3 exit tests (DESIGN.md R10). All are `#[ignore]`; run them with
//! `-- --ignored`, once per instrument (the exit criteria need two from
//! different vendors).
//!
//! ```text
//! RVISA_HW_VXI11=TCPIP0::192.168.1.50::inst0::INSTR \
//! RVISA_HW_BLOCK_CMD="<query returning a binary block>" \
//! RVISA_HW_LARGE_CMD="<query returning >= 1 MiB block>" \
//! RVISA_HW_GOLDEN_DIR=traces/<instrument>/vendor \
//!   cargo test -p rvisa-vxi11 --test hardware -- --ignored --nocapture --test-threads=1
//! ```
//!
//! | Variable | Meaning |
//! |---|---|
//! | `RVISA_HW_VXI11` | resource to test (required) |
//! | `RVISA_HW_IDN_PREFIX`, `RVISA_HW_MULTI_CMD`, `RVISA_HW_BLOCK_CMD`, `RVISA_HW_BLOCK_LEN`, `RVISA_HW_LARGE_CMD`, `RVISA_HW_SRQ_CMD` | as for the vendor hardware tests |
//! | `RVISA_HW_ALLOW` | tests allowed to be unsupported or skipped |
//! | `RVISA_HW_TRACE_DIR` | if set, each test's session is recorded to `<dir>/<test>.jsonl` |
//! | `RVISA_HW_GOLDEN_DIR` | the vendor traces to compare with (`vxi11_matches_golden_traces`) |
//! | `RVISA_HW_VENDOR` | resource for the vendor side of `throughput_1mb` (default: `RVISA_HW_VXI11`) |
//!
//! Commands are given without a terminator; `\n` is appended.
//! `--test-threads=1` keeps the tests from talking to the instrument at the
//! same time. The SRQ test needs the instrument to reach this machine on
//! the interrupt channel: allow inbound TCP through the firewall.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{compare, run_recorded};
use rvisa_conformance::{Profile, TESTS};
use rvisa_core::{Backend, OpenOptions, ResourceName, Session, SessionExt};
use rvisa_mock::Trace;
use rvisa_vendor::VendorBackend;
use rvisa_vxi11::Vxi11Backend;

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

fn hardware_profile() -> Profile {
    let mut profile = Profile::scpi();
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
    if let Some(cmd) = command("RVISA_HW_SRQ_CMD") {
        profile = profile.srq_command(cmd);
    }
    profile
}

fn allowed() -> Vec<String> {
    var("RVISA_HW_ALLOW")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
#[ignore = "needs an instrument (RVISA_HW_VXI11)"]
fn vxi11_conformance() {
    let rn = resource("RVISA_HW_VXI11");
    let backend = Vxi11Backend::new();
    let (report, traces) = run_recorded(
        || backend.open(&rn, &OpenOptions::new()),
        "vxi11",
        &hardware_profile(),
    );
    println!("{rn}:\n{report}");
    if let Some(dir) = var("RVISA_HW_TRACE_DIR").map(PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        for (name, trace) in traces {
            trace.save(dir.join(format!("{name}.jsonl"))).unwrap();
        }
        println!("traces saved to {}", dir.display());
    }
    let allowed = allowed();
    report.assert_conformant(&allowed.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
#[ignore = "needs an instrument (RVISA_HW_VXI11) and vendor traces (RVISA_HW_GOLDEN_DIR)"]
fn vxi11_matches_golden_traces() {
    let rn = resource("RVISA_HW_VXI11");
    let dir = PathBuf::from(var("RVISA_HW_GOLDEN_DIR").expect("set RVISA_HW_GOLDEN_DIR"));
    let golden: Vec<(&str, Trace)> = TESTS
        .iter()
        .filter_map(|(name, _)| {
            let path = dir.join(format!("{name}.jsonl"));
            path.exists().then(|| (*name, Trace::load(&path).unwrap()))
        })
        .collect();
    assert!(!golden.is_empty(), "no traces in {}", dir.display());
    let backend = Vxi11Backend::new();
    let (report, traces) = run_recorded(
        || backend.open(&rn, &OpenOptions::new()),
        "vxi11",
        &hardware_profile(),
    );
    println!("{rn}:\n{report}");
    let diffs = compare(&golden, &traces);
    assert!(
        diffs.is_empty(),
        "differences from the vendor traces:\n{}",
        diffs.join("\n")
    );
}

/// Times `runs` reads of the large block; returns MB/s (best run) and the
/// block size.
fn throughput(session: &mut dyn Session, command: &[u8], runs: usize) -> (f64, usize) {
    session.set_timeout(Some(Duration::from_secs(30))).unwrap();
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

#[test]
#[ignore = "needs an instrument (RVISA_HW_VXI11, RVISA_HW_LARGE_CMD) and a VISA"]
fn throughput_1mb() {
    const RUNS: usize = 5;
    let rn = resource("RVISA_HW_VXI11");
    let vendor_rn = var("RVISA_HW_VENDOR").map_or_else(|| rn.clone(), |v| v.parse().unwrap());
    let command = command("RVISA_HW_LARGE_CMD").expect("set RVISA_HW_LARGE_CMD");
    let mut v = VendorBackend::new()
        .open(&vendor_rn, &OpenOptions::new())
        .unwrap();
    let (vendor_mbps, len) = throughput(v.as_mut(), &command, RUNS);
    drop(v);
    let mut s = Vxi11Backend::new().open(&rn, &OpenOptions::new()).unwrap();
    let (mbps, _) = throughput(s.as_mut(), &command, RUNS);
    println!(
        "{len} bytes, best of {RUNS}: vendor {vendor_mbps:.1} MB/s, vxi11 {mbps:.1} MB/s ({:.0}% of vendor)",
        mbps / vendor_mbps * 100.0
    );
}

#[test]
#[ignore = "needs a network with VXI-11 instruments"]
fn discovery() {
    let found = Vxi11Backend::new()
        .discover(Duration::from_secs(1))
        .unwrap();
    println!("found: {found:#?}");
}
