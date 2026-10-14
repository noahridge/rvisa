//! The vendor VISA backend for `rvisa`: NI-VISA, Keysight VISA, R&S VISA or
//! any other VPP-4.3 implementation, loaded at runtime (ROADMAP D1).
//!
//! Nothing links against VISA at build time, so a binary built with this
//! backend starts on machines without VISA; operations then return
//! `Error::BackendUnavailable` explaining what to install.
//!
//! The library is searched for in this order (DESIGN.md V4):
//!
//! 1. [`VendorBackend::with_library_path`]
//! 2. the `RVISA_VENDOR_LIB` environment variable
//! 3. platform defaults: `visa64.dll`/`visa32.dll` on Windows (DLL search
//!    path, then the VXIplug&play directory from the registry);
//!    `libvisa.so` on Linux; the NI and R&S VISA frameworks on macOS.
//!
//! All `unsafe` code is in one private module (ROADMAP D10, DESIGN.md V1).

#![deny(missing_docs)]
#![deny(unsafe_code)]

mod backend;
mod convert;
#[allow(unsafe_code)]
mod ffi;
mod loader;
mod session;
pub mod visa;

pub use backend::VendorBackend;
pub use loader::{LIBRARY_ENV, candidates};
pub use session::{RenMode, VendorSession};

/// This backend's name.
pub const BACKEND: &str = "vendor";
