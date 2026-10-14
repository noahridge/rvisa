//! Builds the fake VISA library (`rvisa-vendor-fake`) for tests.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

/// Builds `rvisa-vendor-fake` and returns the shared library's path.
pub fn fake_library() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let status = Command::new(env!("CARGO"))
            .args(["build", "-p", "rvisa-vendor-fake"])
            .current_dir(&workspace)
            .status()
            .expect("failed to run cargo");
        assert!(status.success(), "building rvisa-vendor-fake failed");
        let mut target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.join("target"));
        // A child cargo inherits CARGO_BUILD_TARGET, which moves the output.
        if let Some(triple) = std::env::var_os("CARGO_BUILD_TARGET").filter(|t| !t.is_empty()) {
            target.push(triple);
        }
        target.join("debug").join(format!(
            "{}rvisa_vendor_fake{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        ))
    })
}

/// The fake's instruments are process-wide, so tests that use them take
/// this lock to avoid interfering (locks, output queues).
pub fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The demo instruments' conformance profile.
pub fn demo_profile() -> rvisa_conformance::Profile {
    rvisa_conformance::Profile::scpi()
        .idn_prefix(b"ACME Instruments,")
        .multi_line_command(b"MULTI?\n")
        .binary_block_command(b"DATA?\n", Some(8))
        .large_block_command(b"BLOCK:LARGE?\n")
        .srq_command(b"SRQ\n")
}

pub const DEMO: [&str; 5] = [
    "TCPIP0::mock.local::INSTR",
    "TCPIP0::mock.local::5025::SOCKET",
    "USB0::0x1234::0x5678::MOCK001::INSTR",
    "GPIB0::22::INSTR",
    "ASRL1::INSTR",
];
