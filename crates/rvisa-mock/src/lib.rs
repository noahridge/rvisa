//! A scripted mock backend for `rvisa`, plus record and replay sessions.
//!
//! - [`MockBackend`] simulates instruments from a [`Script`], so code and
//!   backends can be tested without hardware. [`MockBackend::demo`] comes
//!   with a set of ready-made SCPI instruments.
//! - [`RecordingSession`] wraps any session and logs every call to a JSON
//!   Lines [`Trace`]; [`ReplaySession`] serves a trace back. Golden traces
//!   recorded on real hardware prove that pure-Rust backends behave like
//!   vendor VISA (ROADMAP §7.3).
//!
//! ```
//! use rvisa_core::{Backend, OpenOptions, SessionExt};
//! use rvisa_mock::{MockBackend, Script};
//!
//! let rn = "TCPIP0::dmm.local::INSTR".parse()?;
//! let backend = MockBackend::new().instrument(
//!     rn,
//!     Script::new().on_write("MEAS?\n").respond(b"+1.25E+00\n"),
//! );
//! let mut dmm = backend.open(&"TCPIP::dmm.local".parse()?, &OpenOptions::new())?;
//! assert_eq!(dmm.query_bytes(b"MEAS?\n")?, b"+1.25E+00\n");
//! # Ok::<(), rvisa_core::Error>(())
//! ```

#![deny(missing_docs)]

mod backend;
mod bytes;
mod record;
mod replay;
mod script;
pub mod trace;

pub use backend::{MockBackend, MockInstrument, MockSession};
pub use bytes::{escape, unescape};
pub use record::RecordingSession;
pub use replay::ReplaySession;
pub use script::{LARGE_BLOCK_LEN, RuleBuilder, Script, demo_script};
pub use trace::Trace;
