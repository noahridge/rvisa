//! The mock backend passes the full conformance suite, and record/replay
//! reproduces it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use rvisa_conformance::{Outcome, Profile, TESTS, run_all};
use rvisa_core::{Backend, OpenOptions, ResourceName, Session};
use rvisa_mock::{LARGE_BLOCK_LEN, MockBackend, RecordingSession, ReplaySession, Trace};

fn profile() -> Profile {
    Profile::scpi()
        .idn_prefix(b"ACME Instruments,")
        .multi_line_command(b"MULTI?\n")
        .binary_block_command(b"DATA?\n", Some(8))
        .large_block_command(b"BLOCK:LARGE?\n")
        .srq_command(b"SRQ\n")
}

const DEMO: [&str; 5] = [
    "TCPIP0::mock.local::INSTR",
    "TCPIP0::mock.local::5025::SOCKET",
    "USB0::0x1234::0x5678::MOCK001::INSTR",
    "GPIB0::22::INSTR",
    "ASRL1::INSTR",
];

#[test]
fn every_demo_instrument_conforms() {
    let backend = MockBackend::demo();
    for name in DEMO {
        let rn: ResourceName = name.parse().unwrap();
        let report = run_all(|| backend.open(&rn, &OpenOptions::new()), &profile());
        assert_eq!(report.results.len(), TESTS.len());
        println!("{name}:\n{report}");
        report.assert_all_passed();
    }
    const { assert!(LARGE_BLOCK_LEN >= 1024 * 1024) };
}

/// A `Write` handle onto a shared buffer, so traces can be collected after
/// each session is dropped by the runner.
#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuf {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn recorded_suite_replays_identically() {
    let backend = MockBackend::demo();
    let rn: ResourceName = "TCPIP0::mock.local::INSTR".parse().unwrap();

    // Record each test's session to its own buffer.
    let buffers: Arc<Mutex<Vec<SharedBuf>>> = Arc::default();
    let recorded = run_all(
        || {
            let buf = SharedBuf::default();
            buffers.lock().unwrap().push(buf.clone());
            let session = backend.open(&rn, &OpenOptions::new())?;
            Ok(Box::new(RecordingSession::new(session, "mock", buf)?) as Box<dyn Session>)
        },
        &profile(),
    );
    recorded.assert_all_passed();

    // Replay them in the same order, with timing for the timeout test.
    let traces: Vec<Trace> = buffers
        .lock()
        .unwrap()
        .iter()
        .map(|b| Trace::read_jsonl(b.0.lock().unwrap().as_slice()).unwrap())
        .collect();
    assert_eq!(traces.len(), TESTS.len());
    let mut next = traces.clone().into_iter();
    let replayed = run_all(
        || {
            let trace = next.next().expect("one trace per test");
            Ok(Box::new(ReplaySession::new(trace)?.realtime()) as Box<dyn Session>)
        },
        &profile(),
    );
    assert_eq!(replayed, recorded);

    // The golden comparison: identical instrument responses.
    let large = &traces[5];
    assert!(large.responses().iter().any(|r| r.len() > LARGE_BLOCK_LEN));
}

#[test]
fn replay_detects_divergence() {
    let backend = MockBackend::demo();
    let rn: ResourceName = "GPIB0::22::INSTR".parse().unwrap();
    let session = backend.open(&rn, &OpenOptions::new()).unwrap();
    let mut rec = RecordingSession::new(session, "mock", Vec::new()).unwrap();
    rec.write(b"*IDN?\n").unwrap();
    let (_, jsonl) = rec.finish().unwrap();

    let trace = Trace::read_jsonl(jsonl.as_slice()).unwrap();
    let mut replay = ReplaySession::new(trace.clone()).unwrap();
    assert_eq!(replay.resource(), &rn);
    // A different payload, then a different operation, then the right call.
    assert!(replay.write(b"*RST\n").is_err());
    assert!(replay.read(10).is_err());
    assert_eq!(replay.remaining(), 1);
    assert_eq!(replay.write(b"*IDN?\n").unwrap(), 6);
    assert!(replay.is_finished());
    let err = replay.clear().unwrap_err();
    assert!(err.to_string().contains("exhausted"), "{err}");

    // A trace must start with an open record.
    let mut headless = trace;
    headless.entries.remove(0);
    assert!(ReplaySession::new(headless).is_err());
}

#[test]
fn outcomes_display() {
    assert_eq!(Outcome::Passed.to_string(), "passed");
    assert_eq!(
        Outcome::Unsupported { op: "read_stb" }.to_string(),
        "unsupported (read_stb)"
    );
}
