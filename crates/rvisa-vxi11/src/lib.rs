//! A pure-Rust VXI-11 backend for `rvisa`:
//! `TCPIP[board]::host[::device][::INSTR]` resources (ROADMAP Phase 3),
//! the protocol most LXI instruments speak.
//!
//! VXI-11 runs over ONC-RPC. This crate has its own minimal RPC and XDR
//! code (DESIGN.md R2) and no other dependencies beyond `socket2`:
//!
//! - `open` asks the instrument's portmapper for the core channel's port,
//!   connects, and creates a link to the device (`inst0` unless the
//!   resource names another, such as `gpib0,5`).
//! - Writes are split by the device's `maxRecvSize`, with END on the last
//!   part. Reads end at END, at the term char if enabled, or after the
//!   requested count; a timeout carries the bytes received.
//! - Device clear, status byte, trigger and exclusive locks are native
//!   VXI-11 calls. An operation stuck past its timeout is cancelled
//!   through the abort channel ([`AbortHandle`]).
//! - With the `srq` feature, service request events arrive through the
//!   interrupt channel, which the session serves itself.
//! - [`Vxi11Backend::discover`] finds instruments with a portmapper
//!   broadcast.
//!
//! ```no_run
//! use rvisa_core::{Backend, OpenOptions, SessionExt};
//! use rvisa_vxi11::Vxi11Backend;
//!
//! let backend = Vxi11Backend::new();
//! let mut dmm = backend.open(&"TCPIP0::192.168.1.50::inst0::INSTR".parse()?, &OpenOptions::new())?;
//! let idn = dmm.query_bytes(b"*IDN?\n")?;
//! # Ok::<(), rvisa_core::Error>(())
//! ```
//!
//! With the `server` feature, [`Vxi11Server`] serves any session as a
//! VXI-11 instrument on a local port, for tests.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod backend;
mod portmap;
mod proto;
mod rpc;
#[cfg(feature = "server")]
mod server;
mod session;
mod settings;
#[cfg(feature = "srq")]
mod srq;
mod xdr;

pub use backend::{DEFAULT_DEVICE, Vxi11Backend};
#[cfg(feature = "server")]
pub use server::Vxi11Server;
pub use session::{AbortHandle, Vxi11Session};
pub use settings::{DEFAULT_CONNECT_TIMEOUT, DEFAULT_TIMEOUT};

/// This backend's name.
pub const BACKEND: &str = "vxi11";

/// Entry points for the fuzz targets. Not part of the API.
#[doc(hidden)]
pub mod fuzzing {
    /// Decodes `data` as every VXI-11 structure, RPC reply and call, and
    /// as a stream of RPC records. Must never panic.
    pub fn decode_all(data: &[u8]) {
        crate::proto::decode_any(data);
        let _ = crate::rpc::decode_reply(data);
        let _ = crate::rpc::decode_call(data);
        let mut buf = data.to_vec();
        while let Ok(Some(_)) = crate::rpc::take_record(&mut buf, 1 << 20) {}
    }
}
