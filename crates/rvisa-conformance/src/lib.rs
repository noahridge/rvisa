//! A backend-agnostic conformance suite for `rvisa` backends (ROADMAP §7.2).
//!
//! Every backend runs [`run_all`] in its own tests: against the mock in CI,
//! and against real instruments in `#[ignore]` tests gated by environment
//! variables. Because instruments differ, each test takes a [`Profile`]
//! naming the commands to send.
//!
//! Tests never pass silently. Each returns an [`Outcome`]: a backend that
//! cannot perform an operation reports [`Outcome::Unsupported`], and a test
//! whose command the profile lacks reports [`Outcome::Skipped`].
//!
//! ```
//! use rvisa_conformance::{Profile, run_all};
//! # use rvisa_core::{Backend, OpenOptions, Session};
//! # fn open() -> rvisa_core::Result<Box<dyn Session>> {
//! #     let backend = rvisa_mock::MockBackend::demo();
//! #     backend.open(&"GPIB0::22::INSTR".parse()?, &OpenOptions::new())
//! # }
//!
//! let profile = Profile::scpi()
//!     .multi_line_command(b"MULTI?\n")
//!     .binary_block_command(b"DATA?\n", Some(8))
//!     .large_block_command(b"BLOCK:LARGE?\n")
//!     .srq_command(b"SRQ\n");
//! let report = run_all(open, &profile);
//! report.assert_all_passed();
//! ```

#![deny(missing_docs)]

use std::fmt;
use std::time::{Duration, Instant};

use rvisa_core::{
    Attr, AttrValue, Error, EventKind, LockKind, READ_CHUNK, ResourceName, Result, Session,
    SessionExt, TermReason,
};

/// The commands and settings the suite uses for one instrument.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// The identification query, including its terminator.
    pub idn_command: Vec<u8>,
    /// If set, the identification response must start with this.
    pub idn_prefix: Option<Vec<u8>>,
    /// A query whose single response message contains the term char before
    /// its end (for example two lines).
    pub multi_line_command: Option<Vec<u8>>,
    /// A query answered with an IEEE 488.2 definite-length block.
    pub binary_block_command: Option<Vec<u8>>,
    /// The expected length of that block's data, if known.
    pub binary_block_len: Option<usize>,
    /// A query answered with a block of at least 1 MiB.
    pub large_block_command: Option<Vec<u8>>,
    /// A command that makes the instrument request service (SRQ).
    pub srq_command: Option<Vec<u8>>,
    /// The read termination character the instrument uses.
    pub term_char: u8,
    /// The I/O timeout for the tests.
    pub io_timeout: Duration,
}

impl Profile {
    /// A generic SCPI instrument: `*IDN?\n`, newline termination, a 5 s
    /// timeout, and no optional commands.
    #[must_use]
    pub fn scpi() -> Self {
        Self {
            idn_command: b"*IDN?\n".to_vec(),
            idn_prefix: None,
            multi_line_command: None,
            binary_block_command: None,
            binary_block_len: None,
            large_block_command: None,
            srq_command: None,
            term_char: b'\n',
            io_timeout: Duration::from_secs(5),
        }
    }

    /// Requires the identification response to start with `prefix`.
    #[must_use]
    pub fn idn_prefix(mut self, prefix: impl AsRef<[u8]>) -> Self {
        self.idn_prefix = Some(prefix.as_ref().to_vec());
        self
    }

    /// Sets [`multi_line_command`](Self::multi_line_command).
    #[must_use]
    pub fn multi_line_command(mut self, command: impl AsRef<[u8]>) -> Self {
        self.multi_line_command = Some(command.as_ref().to_vec());
        self
    }

    /// Sets [`binary_block_command`](Self::binary_block_command) and the
    /// expected data length.
    #[must_use]
    pub fn binary_block_command(mut self, command: impl AsRef<[u8]>, len: Option<usize>) -> Self {
        self.binary_block_command = Some(command.as_ref().to_vec());
        self.binary_block_len = len;
        self
    }

    /// Sets [`large_block_command`](Self::large_block_command).
    #[must_use]
    pub fn large_block_command(mut self, command: impl AsRef<[u8]>) -> Self {
        self.large_block_command = Some(command.as_ref().to_vec());
        self
    }

    /// Sets [`srq_command`](Self::srq_command).
    #[must_use]
    pub fn srq_command(mut self, command: impl AsRef<[u8]>) -> Self {
        self.srq_command = Some(command.as_ref().to_vec());
        self
    }

    /// Sets the I/O timeout used by the tests.
    #[must_use]
    pub fn io_timeout(mut self, timeout: Duration) -> Self {
        self.io_timeout = timeout;
        self
    }
}

/// The result of one conformance test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The backend behaved as required.
    Passed,
    /// The backend returned `Error::Unsupported` for `op`.
    Unsupported {
        /// The unsupported operation.
        op: &'static str,
    },
    /// The test could not run, for example because the profile lacks a
    /// command it needs.
    Skipped {
        /// Why.
        reason: String,
    },
    /// The backend misbehaved.
    Failed {
        /// What went wrong.
        reason: String,
    },
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Passed => f.write_str("passed"),
            Self::Unsupported { op } => write!(f, "unsupported ({op})"),
            Self::Skipped { reason } => write!(f, "skipped: {reason}"),
            Self::Failed { reason } => write!(f, "FAILED: {reason}"),
        }
    }
}

/// Why a test body stopped early.
enum Stop {
    Error(Error),
    Failed(String),
    Skipped(String),
}

impl From<Error> for Stop {
    fn from(e: Error) -> Self {
        Self::Error(e)
    }
}

type Check = std::result::Result<(), Stop>;

fn conclude(result: Check) -> Outcome {
    match result {
        Ok(()) => Outcome::Passed,
        Err(Stop::Error(Error::Unsupported { op, .. })) => Outcome::Unsupported { op },
        Err(Stop::Error(e)) => Outcome::Failed {
            reason: e.to_string(),
        },
        Err(Stop::Failed(reason)) => Outcome::Failed { reason },
        Err(Stop::Skipped(reason)) => Outcome::Skipped { reason },
    }
}

macro_rules! ensure {
    ($cond:expr, $($msg:tt)+) => {
        if !$cond {
            return Err(Stop::Failed(format!($($msg)+)));
        }
    };
}

fn needs<'p>(command: Option<&'p Vec<u8>>, what: &str) -> std::result::Result<&'p [u8], Stop> {
    command
        .map(Vec::as_slice)
        .ok_or_else(|| Stop::Skipped(format!("the profile has no {what}")))
}

/// Puts a fresh session into a known state: the profile's timeout, no read
/// termination, and (where supported) a device clear to discard stale
/// output.
fn prepare(s: &mut dyn Session, p: &Profile) -> Check {
    s.set_timeout(Some(p.io_timeout))?;
    s.set_read_termination(None)?;
    match s.clear() {
        Ok(()) | Err(Error::Unsupported { .. }) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn show(data: &[u8]) -> String {
    String::from_utf8_lossy(data).escape_debug().to_string()
}

/// `*IDN?` (or the profile's query) answers with a non-empty message, the
/// same way twice.
pub fn idn_roundtrip(s: &mut dyn Session, p: &Profile) -> Outcome {
    conclude((|| {
        prepare(s, p)?;
        let first = s.query_bytes(&p.idn_command)?;
        ensure!(!first.is_empty(), "empty identification response");
        if let Some(prefix) = &p.idn_prefix {
            ensure!(
                first.starts_with(prefix),
                "response {:?} does not start with {:?}",
                show(&first),
                show(prefix)
            );
        }
        s.write_all(&p.idn_command)?;
        let second = s.read_message()?;
        ensure!(
            second == first,
            "responses differ: {:?} then {:?}",
            show(&first),
            show(&second)
        );
        Ok(())
    })())
}

/// A read smaller than the response stops with `MaxCount`, and the rest of
/// the message follows.
pub fn read_max_count_then_rest(s: &mut dyn Session, p: &Profile) -> Outcome {
    conclude((|| {
        prepare(s, p)?;
        let full = s.query_bytes(&p.idn_command)?;
        if full.len() <= 4 {
            return Err(Stop::Skipped("identification response is too short".into()));
        }
        s.write_all(&p.idn_command)?;
        let head = s.read(4)?;
        ensure!(
            head.reason == TermReason::MaxCount && head.data.len() == 4,
            "read(4) returned {} bytes with {:?}",
            head.data.len(),
            head.reason
        );
        let mut joined = head.data;
        joined.extend(s.read_message()?);
        ensure!(
            joined == full,
            "pieces {:?} do not rebuild {:?}",
            show(&joined),
            show(&full)
        );
        Ok(())
    })())
}

/// A read with nothing to receive fails with `Timeout` after about the
/// configured time.
pub fn timeout_is_respected(s: &mut dyn Session, p: &Profile) -> Outcome {
    const TIMEOUT: Duration = Duration::from_millis(200);
    const SLACK: Duration = Duration::from_secs(2);
    conclude((|| {
        prepare(s, p)?;
        s.set_timeout(Some(TIMEOUT))?;
        ensure!(
            s.timeout() == Some(TIMEOUT),
            "timeout() is {:?}",
            s.timeout()
        );
        let start = Instant::now();
        let result = s.read(64);
        let elapsed = start.elapsed();
        match result {
            Err(Error::Timeout) => {}
            Err(e) => return Err(e.into()),
            Ok(r) => {
                return Err(Stop::Failed(format!(
                    "read returned {:?} with nothing pending",
                    show(&r.data)
                )));
            }
        }
        ensure!(
            elapsed >= TIMEOUT * 3 / 4 && elapsed <= TIMEOUT + SLACK,
            "timed out after {elapsed:?} for a {TIMEOUT:?} timeout"
        );
        Ok(())
    })())
}

/// With the term char enabled, a read stops at it with `TermChar`, before
/// the end of the message.
pub fn read_termination_char(s: &mut dyn Session, p: &Profile) -> Outcome {
    conclude((|| {
        let command = needs(p.multi_line_command.as_ref(), "multi-line command")?;
        prepare(s, p)?;
        s.set_read_termination(Some(p.term_char))?;
        s.write_all(command)?;
        let line = s.read(READ_CHUNK)?;
        ensure!(
            line.reason == TermReason::TermChar,
            "first read ended with {:?}",
            line.reason
        );
        ensure!(
            line.data.last() == Some(&p.term_char),
            "first read {:?} does not end with the term char",
            show(&line.data)
        );
        let rest = s.read_message()?;
        ensure!(!rest.is_empty(), "nothing followed the first line");
        s.set_read_termination(None)?;
        Ok(())
    })())
}

/// A definite-length block is read whole.
pub fn binary_block_definite_length(s: &mut dyn Session, p: &Profile) -> Outcome {
    conclude((|| {
        let command = needs(p.binary_block_command.as_ref(), "binary block command")?;
        prepare(s, p)?;
        s.write_all(command)?;
        let block = s.read_ieee488_block()?;
        if let Some(len) = p.binary_block_len {
            ensure!(
                block.len() == len,
                "block has {} bytes, expected {len}",
                block.len()
            );
        }
        // The stream is still in step afterwards.
        ensure!(
            !s.query_bytes(&p.idn_command)?.is_empty(),
            "no response after the block"
        );
        Ok(())
    })())
}

/// A block of at least 1 MiB arrives intact, even with the term char
/// enabled (binary data contains it).
pub fn large_transfer_1mb(s: &mut dyn Session, p: &Profile) -> Outcome {
    conclude((|| {
        let command = needs(p.large_block_command.as_ref(), "large block command")?;
        prepare(s, p)?;
        s.set_read_termination(Some(p.term_char))?;
        s.write_all(command)?;
        let block = s.read_ieee488_block()?;
        ensure!(
            block.len() >= 1024 * 1024,
            "block has only {} bytes",
            block.len()
        );
        let idn = s.query_bytes(&p.idn_command)?;
        ensure!(!idn.is_empty(), "no response after the block");
        s.set_read_termination(None)?;
        Ok(())
    })())
}

/// After a partial read, a device clear discards the rest, and the next
/// query is answered normally.
pub fn device_clear_recovers_after_partial_read(s: &mut dyn Session, p: &Profile) -> Outcome {
    conclude((|| {
        prepare(s, p)?;
        let expected = s.query_bytes(&p.idn_command)?;
        s.write_all(&p.idn_command)?;
        let partial = s.read(2)?;
        ensure!(
            partial.reason == TermReason::MaxCount,
            "read(2) ended with {:?}",
            partial.reason
        );
        s.clear()?;
        let after = s.query_bytes(&p.idn_command)?;
        ensure!(
            after == expected,
            "after clear got {:?}, expected {:?}",
            show(&after),
            show(&expected)
        );
        Ok(())
    })())
}

/// The SRQ command raises a service request event, and the status byte has
/// RQS (0x40) set.
pub fn stb_and_srq_event(s: &mut dyn Session, p: &Profile) -> Outcome {
    conclude((|| {
        let command = needs(p.srq_command.as_ref(), "SRQ command")?;
        prepare(s, p)?;
        s.enable_event(EventKind::ServiceRequest)?;
        s.write_all(command)?;
        let event = s.wait_event(EventKind::ServiceRequest, p.io_timeout)?;
        ensure!(
            event.kind == EventKind::ServiceRequest,
            "got {:?}",
            event.kind
        );
        let stb = s.read_stb()?;
        ensure!(stb & 0x40 != 0, "status byte {stb:#04x} lacks RQS");
        s.disable_event(EventKind::ServiceRequest)?;
        Ok(())
    })())
}

/// Waiting for an event that is not enabled fails with `EventNotEnabled`.
pub fn wait_event_requires_enable(s: &mut dyn Session, p: &Profile) -> Outcome {
    conclude((|| {
        prepare(s, p)?;
        match s.wait_event(EventKind::ServiceRequest, Duration::from_millis(50)) {
            Err(Error::EventNotEnabled(EventKind::ServiceRequest)) => Ok(()),
            Err(e @ Error::Unsupported { .. }) => Err(e.into()),
            other => Err(Stop::Failed(format!(
                "expected EventNotEnabled, got {other:?}"
            ))),
        }
    })())
}

/// A software trigger is accepted.
pub fn assert_trigger(s: &mut dyn Session, p: &Profile) -> Outcome {
    conclude((|| {
        prepare(s, p)?;
        s.assert_trigger()?;
        Ok(())
    })())
}

/// An exclusive lock can be taken, used, and released once.
pub fn lock_and_unlock(s: &mut dyn Session, p: &Profile) -> Outcome {
    conclude((|| {
        prepare(s, p)?;
        s.lock(LockKind::Exclusive, p.io_timeout)?;
        let idn = s.query_bytes(&p.idn_command);
        s.unlock()?;
        ensure!(
            idn.is_ok_and(|r| !r.is_empty()),
            "query failed while locked"
        );
        ensure!(s.unlock().is_err(), "a second unlock succeeded");
        Ok(())
    })())
}

/// Attributes agree with the session's own settings, reject writes to
/// read-only attributes, and reject values of the wrong type.
pub fn attributes_roundtrip(s: &mut dyn Session, p: &Profile) -> Outcome {
    conclude((|| {
        prepare(s, p)?;
        let timeout = Duration::from_millis(1500);
        s.set_attr(Attr::TimeoutValue, AttrValue::Timeout(Some(timeout)))?;
        ensure!(
            s.timeout() == Some(timeout),
            "timeout() is {:?} after set_attr",
            s.timeout()
        );
        let read = s.get_attr(Attr::TimeoutValue)?;
        ensure!(
            read == AttrValue::Timeout(Some(timeout)),
            "TimeoutValue reads {read:?}"
        );

        let name = s.get_attr(Attr::ResourceName)?;
        let parses = name
            .as_str()
            .is_some_and(|n| n.parse::<ResourceName>().is_ok());
        ensure!(parses, "ResourceName {name:?} is not a resource name");

        match s.set_attr(Attr::ResourceName, AttrValue::Str("x".into())) {
            Err(Error::AttrReadOnly(Attr::ResourceName)) => {}
            other => return Err(Stop::Failed(format!("writing ResourceName gave {other:?}"))),
        }
        match s.set_attr(Attr::TermChar, AttrValue::Bool(true)) {
            Err(Error::InvalidArgument(_)) => {}
            other => return Err(Stop::Failed(format!("a Bool TermChar gave {other:?}"))),
        }
        Ok(())
    })())
}

/// A conformance test.
pub type Test = fn(&mut dyn Session, &Profile) -> Outcome;

/// Every test in the suite, by name.
pub const TESTS: &[(&str, Test)] = &[
    ("idn_roundtrip", idn_roundtrip),
    ("read_max_count_then_rest", read_max_count_then_rest),
    ("timeout_is_respected", timeout_is_respected),
    ("read_termination_char", read_termination_char),
    ("binary_block_definite_length", binary_block_definite_length),
    ("large_transfer_1mb", large_transfer_1mb),
    (
        "device_clear_recovers_after_partial_read",
        device_clear_recovers_after_partial_read,
    ),
    ("stb_and_srq_event", stb_and_srq_event),
    ("wait_event_requires_enable", wait_event_requires_enable),
    ("assert_trigger", assert_trigger),
    ("lock_and_unlock", lock_and_unlock),
    ("attributes_roundtrip", attributes_roundtrip),
];

/// Runs every test, each on a fresh session from `open`.
pub fn run_all(mut open: impl FnMut() -> Result<Box<dyn Session>>, profile: &Profile) -> Report {
    let results = TESTS
        .iter()
        .map(|&(name, test)| {
            let outcome = match open() {
                Ok(mut session) => test(session.as_mut(), profile),
                Err(e) => Outcome::Failed {
                    reason: format!("could not open a session: {e}"),
                },
            };
            (name, outcome)
        })
        .collect();
    Report { results }
}

/// The results of [`run_all`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Each test's name and outcome, in suite order.
    pub results: Vec<(&'static str, Outcome)>,
}

impl Report {
    /// Tests that did not pass, excluding `Unsupported` outcomes for the
    /// named tests.
    #[must_use]
    pub fn problems(&self, allowed_unsupported: &[&str]) -> Vec<(&'static str, &Outcome)> {
        self.results
            .iter()
            .filter(|(name, outcome)| match outcome {
                Outcome::Passed => false,
                Outcome::Unsupported { .. } => !allowed_unsupported.contains(name),
                _ => true,
            })
            .map(|(name, outcome)| (*name, outcome))
            .collect()
    }

    /// Panics unless every test passed.
    ///
    /// # Panics
    ///
    /// If any test did not pass; the message lists the full report.
    pub fn assert_all_passed(&self) {
        self.assert_conformant(&[]);
    }

    /// Panics unless every test passed, except that the named tests may be
    /// `Unsupported`.
    ///
    /// # Panics
    ///
    /// If any other test did not pass; the message lists the full report.
    pub fn assert_conformant(&self, allowed_unsupported: &[&str]) {
        assert!(
            self.problems(allowed_unsupported).is_empty(),
            "conformance failures:\n{self}"
        );
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (name, outcome) in &self.results {
            writeln!(f, "  {name}: {outcome}")?;
        }
        Ok(())
    }
}
