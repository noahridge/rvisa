//! The facade with the vendor backend, against the fake VISA library.

#![cfg(feature = "vendor")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use rvisa::prelude::*;
use rvisa::vendor::VendorBackend;
use rvisa::{Backend, Error};
use rvisa_mock::MockBackend;

fn fake_library() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let status = Command::new(env!("CARGO"))
            .args(["build", "-p", "rvisa-vendor-fake"])
            .current_dir(&workspace)
            .status()
            .unwrap();
        assert!(status.success());
        let mut target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.join("target"));
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

/// Whether a real VISA is installed here (some assertions only hold
/// without one).
fn real_visa_installed() -> bool {
    VendorBackend::with_library_path("/nonexistent")
        .load()
        .is_ok()
}

#[test]
fn vendor_library_option_and_alias() {
    let rm = ResourceManager::builder()
        .no_default_backends()
        .backend(VendorBackend::with_library_path(fake_library()))
        .build()
        .unwrap();
    let mut dmm = rm.open("TCPIP0::mock.local::INSTR").unwrap();
    assert_eq!(dmm.backend(), "vendor");
    assert!(dmm.query("*IDN?").unwrap().starts_with("ACME Instruments"));

    // Not a resource string: resolved through viParseRsrcEx.
    let mut aliased = rm.open("mockdmm").unwrap();
    assert_eq!(aliased.resource().to_string(), "TCPIP0::mock.local::INSTR");
    assert!(aliased.query("*IDN?").is_ok());
    assert!(matches!(
        rm.open("no such alias"),
        Err(Error::InvalidResource(_))
    ));

    let rm = ResourceManager::builder()
        .vendor_library(fake_library())
        .build()
        .unwrap();
    assert!(rm.backend_names().contains(&"vendor"));
    assert_eq!(rm.list("USB?*").unwrap().len(), 1);
}

#[test]
fn unavailable_vendor_does_not_hide_other_backends() {
    if real_visa_installed() {
        return;
    }
    let missing = || VendorBackend::with_library_path("/nonexistent/libvisa");
    let rm = ResourceManager::builder()
        .no_default_backends()
        .backend(MockBackend::demo())
        .backend(missing())
        .build()
        .unwrap();
    assert_eq!(rm.list("?*").unwrap().len(), 5);
    match rm.open("TCPIP0::10.0.0.99::INSTR") {
        Err(Error::BackendUnavailable {
            backend: "vendor",
            reason,
        }) => {
            assert!(reason.contains("RVISA_VENDOR_LIB"), "{reason}");
        }
        other => panic!("expected BackendUnavailable, got {other:?}"),
    }

    let only_vendor = ResourceManager::builder()
        .no_default_backends()
        .backend(missing())
        .build()
        .unwrap();
    assert!(matches!(
        only_vendor.list("?*"),
        Err(Error::BackendUnavailable { .. })
    ));
    assert!(matches!(
        only_vendor.open("dmm"),
        Err(Error::InvalidResource(_))
    ));
    assert!(missing().supports(&"GPIB0::1".parse().unwrap()));
}
