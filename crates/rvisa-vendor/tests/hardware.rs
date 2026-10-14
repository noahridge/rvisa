//! Tests against a real VISA installation. All are `#[ignore]`; run them
//! with `-- --ignored`. The library is found as usual (the defaults, or
//! `RVISA_VENDOR_LIB`).
//!
//! # Against an instrument (Phase 1 task 7)
//!
//! ```text
//! RVISA_HW_VENDOR=TCPIP0::192.168.1.50::INSTR \
//! RVISA_HW_BLOCK_CMD="CURV?" RVISA_HW_SRQ_CMD="*SRE 32;*ESE 1;*OPC" \
//! RVISA_HW_TRACE_DIR=traces/keithley-3700a \
//!   cargo test -p rvisa-vendor --test hardware -- --ignored --nocapture vendor_conformance
//! ```
//!
//! | Variable | Meaning |
//! |---|---|
//! | `RVISA_HW_VENDOR` | resource to test (required) |
//! | `RVISA_HW_IDN_PREFIX` | expected start of the `*IDN?` response |
//! | `RVISA_HW_MULTI_CMD` | a query whose response contains a newline before its end |
//! | `RVISA_HW_BLOCK_CMD`, `RVISA_HW_BLOCK_LEN` | a definite-length block query, and its data length |
//! | `RVISA_HW_LARGE_CMD` | a query answered with a block of at least 1 MiB |
//! | `RVISA_HW_SRQ_CMD` | a command that makes the instrument assert SRQ |
//! | `RVISA_HW_TERM_ALWAYS` | `1` for sockets (no END indicator) |
//! | `RVISA_HW_ALLOW` | comma-separated tests allowed to be unsupported or skipped |
//! | `RVISA_HW_TRACE_DIR` | if set, each test's session is recorded to `<dir>/<test>.jsonl` |
//!
//! Commands are given without a terminator; `\n` is appended.
//!
//! # Without an instrument
//!
//! `vendor_against_tcp_simulator` needs only a VISA installed: it serves the
//! mock demo instrument on a local TCP port and opens it through VISA as a
//! `SOCKET` resource.
//!
//! ```text
//! cargo test -p rvisa-vendor --test hardware -- --ignored --nocapture vendor_against_tcp_simulator
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use rvisa_conformance::{Profile, TESTS, run_all};
use rvisa_core::{Backend, OpenOptions, ResourceName, Session};
use rvisa_mock::{RecordingSession, TcpInstrument, demo_script};
use rvisa_vendor::VendorBackend;

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn command(name: &str) -> Option<Vec<u8>> {
    var(name).map(|c| format!("{c}\n").into_bytes())
}

fn hardware_profile() -> Profile {
    let mut profile =
        Profile::scpi().term_char_always(var("RVISA_HW_TERM_ALWAYS").as_deref() == Some("1"));
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

#[test]
#[ignore = "needs VISA and an instrument (RVISA_HW_VENDOR)"]
fn vendor_conformance() {
    let resource: ResourceName = var("RVISA_HW_VENDOR")
        .expect("set RVISA_HW_VENDOR to the resource to test")
        .parse()
        .unwrap();
    let visa = VendorBackend::new();
    println!("VISA: {}", visa.library_path().unwrap());
    println!("missing functions: {:?}", visa.missing_functions().unwrap());

    let trace_dir = var("RVISA_HW_TRACE_DIR").map(PathBuf::from);
    if let Some(dir) = &trace_dir {
        std::fs::create_dir_all(dir).unwrap();
    }
    let index = AtomicUsize::new(0);
    let report = run_all(
        || {
            let session = visa.open(&resource, &OpenOptions::new())?;
            let i = index.fetch_add(1, Ordering::Relaxed);
            match &trace_dir {
                Some(dir) => {
                    let path = dir.join(format!("{}.jsonl", TESTS[i].0));
                    let file = std::fs::File::create(path)?;
                    Ok(Box::new(RecordingSession::new(session, "vendor", file)?)
                        as Box<dyn Session>)
                }
                None => Ok(session),
            }
        },
        &hardware_profile(),
    );
    println!("{resource}:\n{report}");
    let allowed = var("RVISA_HW_ALLOW").unwrap_or_default();
    let allowed: Vec<&str> = allowed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    report.assert_conformant(&allowed);
}

#[test]
#[ignore = "needs a VISA installed"]
fn vendor_timeout_keeps_partial_data() {
    use std::io::Write;
    use std::time::Duration;

    // DESIGN.md C12: VISA reports the bytes that arrived before a timeout.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let rn: ResourceName = format!("TCPIP0::127.0.0.1::{port}::SOCKET")
        .parse()
        .unwrap();
    let visa = VendorBackend::new();
    let mut s = visa.open(&rn, &OpenOptions::new()).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    s.set_timeout(Some(Duration::from_millis(300))).unwrap();
    s.set_read_termination(Some(b'\n')).unwrap();
    peer.write_all(b"PARTIAL").unwrap();
    match s.read(100) {
        Err(rvisa_core::Error::Timeout { partial, .. }) => assert_eq!(partial, b"PARTIAL"),
        other => panic!("expected a timeout with data, got {other:?}"),
    }
    peer.write_all(b"\n").unwrap();
    assert_eq!(s.read(100).unwrap().data, b"\n", "the bytes were consumed");
}

#[test]
#[ignore = "needs a VISA installed"]
fn vendor_against_tcp_simulator() {
    let server = TcpInstrument::spawn(demo_script("SIM100", "SIM001")).unwrap();
    let visa = VendorBackend::new();
    println!("VISA: {}", visa.library_path().unwrap());
    let profile = Profile::scpi()
        .term_char_always(true)
        .idn_prefix(b"ACME Instruments,SIM100")
        .multi_line_command(b"MULTI?\n")
        .binary_block_command(b"DATA?\n", Some(8))
        .large_block_command(b"BLOCK:LARGE?\n");
    let report = run_all(
        || visa.open(server.resource(), &OpenOptions::new()),
        &profile,
    );
    println!("{}:\n{report}", server.resource());
    // A raw socket has no SRQ, and VISA may not support every operation on
    // it; anything else must pass.
    report.assert_conformant(&[
        "stb_and_srq_event",
        "wait_event_requires_enable",
        "device_clear_recovers_after_partial_read",
        "assert_trigger",
        "lock_and_unlock",
    ]);
}
