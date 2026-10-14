//! A pure-Rust raw TCP socket backend for `rvisa`:
//! `TCPIP[board]::host::port::SOCKET` resources (ROADMAP Phase 2).
//!
//! It behaves like NI-VISA on the same resources (DESIGN.md S1–S8):
//!
//! - Writes send bytes unchanged.
//! - Reads end at the term char, if enabled, or after the requested count.
//!   A raw socket has no END, so without a term char a read ends in a
//!   timeout that carries the bytes received.
//! - Device clear discards input already received, then optionally sends
//!   [`SocketBackend::clear_command`].
//! - `read_stb` and `assert_trigger` are unsupported unless 488.2 string
//!   emulation sends `*STB?` and `*TRG` ([`SocketBackend::emulate_488_strings`]).
//! - Locks work between sessions of one backend; events are unsupported.
//!
//! With the `tokio` feature, [`tokio::AsyncSocketSession`] is a native async
//! session with the same behaviour.
//!
//! ```no_run
//! use rvisa_core::{Backend, OpenOptions, Session, SessionExt};
//! use rvisa_socket::SocketBackend;
//!
//! let backend = SocketBackend::new();
//! let mut dmm = backend.open(&"TCPIP0::192.168.1.50::5025::SOCKET".parse()?, &OpenOptions::new())?;
//! dmm.set_read_termination(Some(b'\n'))?;
//! let idn = dmm.query_bytes(b"*IDN?\n")?;
//! # Ok::<(), rvisa_core::Error>(())
//! ```

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod backend;
mod buffer;
mod session;
mod settings;
#[cfg(feature = "tokio")]
pub mod tokio;

pub use backend::SocketBackend;
pub use buffer::ReadBuffer;
pub use session::SocketSession;
pub use settings::{
    ATTR_IO_PROT, DEFAULT_CONNECT_TIMEOUT, DEFAULT_TIMEOUT, PROT_4882_STRS, PROT_NORMAL,
};

/// This backend's name.
pub const BACKEND: &str = "socket";
