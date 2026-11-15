//! Helpers shared by the VXI-11 backend's tests.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};

use rvisa_conformance::{Profile, Report, TESTS, run_all};
use rvisa_core::{Backend, OpenOptions, ResourceName, Result, Session};
use rvisa_mock::{MockBackend, MockInstrument, RecordingSession, Trace, demo_script};
use rvisa_vxi11::{Vxi11Backend, Vxi11Server};

/// A mock instrument running the demo script, served over VXI-11 as
/// device `inst0`. Other device names are not found.
pub struct Sim {
    pub server: Vxi11Server,
    pub instrument: MockInstrument,
}

impl Sim {
    pub fn new() -> Self {
        let rn: ResourceName = "TCPIP0::sim::INSTR".parse().unwrap();
        let mock =
            Arc::new(MockBackend::new().instrument(rn.clone(), demo_script("SIM100", "SIM001")));
        let instrument = mock.handle(&rn).unwrap();
        let server = Vxi11Server::spawn(move |device| {
            if device == "inst0" {
                mock.open(&rn, &OpenOptions::new())
            } else {
                Err(rvisa_core::Error::NotFound(rn.clone()))
            }
        })
        .unwrap();
        Self { server, instrument }
    }

    /// A backend pointed at this server.
    pub fn backend(&self) -> Vxi11Backend {
        Vxi11Backend::new().portmapper_port(self.server.port())
    }

    pub fn resource(&self) -> &ResourceName {
        self.server.resource()
    }
}

/// The demo instrument's conformance profile. VXI-11 has END, so the term
/// char is only enabled where a test needs it.
pub fn demo_profile() -> Profile {
    Profile::scpi()
        .idn_prefix(b"ACME Instruments,SIM100")
        .multi_line_command(b"MULTI?\n")
        .binary_block_command(b"DATA?\n", Some(8))
        .large_block_command(b"BLOCK:LARGE?\n")
        .srq_command(b"SRQ\n")
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
) -> (Report, Vec<(&'static str, Trace)>) {
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
    let traces = TESTS
        .iter()
        .zip(&bufs)
        .map(|((name, _), b)| {
            let bytes = b.0.lock().unwrap().clone();
            (*name, Trace::read_jsonl(bytes.as_slice()).unwrap())
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
