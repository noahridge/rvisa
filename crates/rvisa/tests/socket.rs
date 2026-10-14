//! The facade with the socket backend (DESIGN.md F3).

#![cfg(feature = "socket")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rvisa::prelude::*;
use rvisa::socket::SocketBackend;
use rvisa::{Backend, Error, OpenOptions, ResourceName};
use rvisa_mock::{TcpInstrument, demo_script};

/// A backend that supports everything but is never available, like the
/// vendor backend on a machine without VISA.
struct Unavailable;

impl Backend for Unavailable {
    fn name(&self) -> &'static str {
        "unavailable"
    }
    fn supports(&self, _: &ResourceName) -> bool {
        true
    }
    fn open(&self, _: &ResourceName, _: &OpenOptions) -> rvisa::Result<Box<dyn Session>> {
        Err(Error::BackendUnavailable {
            backend: "unavailable",
            reason: "not installed".into(),
        })
    }
    fn list(&self, _: &str) -> rvisa::Result<Vec<ResourceName>> {
        Err(Error::BackendUnavailable {
            backend: "unavailable",
            reason: "not installed".into(),
        })
    }
}

#[test]
fn socket_is_registered_by_default() {
    let rm = ResourceManager::new().unwrap();
    assert!(rm.backend_names().contains(&"socket"));
}

#[test]
fn an_unavailable_backend_falls_through_to_socket() {
    let server = TcpInstrument::spawn(demo_script("SIM100", "SIM001")).unwrap();
    let rm = ResourceManager::builder()
        .no_default_backends()
        .backend(Unavailable)
        .backend(SocketBackend::new())
        .build()
        .unwrap();
    let mut dmm = rm.open(&server.resource().to_string()).unwrap();
    assert_eq!(dmm.backend(), "socket");
    dmm.set_read_termination(Some(b'\n')).unwrap();
    assert!(
        dmm.query("*IDN?")
            .unwrap()
            .starts_with("ACME Instruments,SIM100")
    );

    // Forcing a backend never falls back.
    assert!(matches!(
        rm.open_with(&server.resource().to_string(), "unavailable"),
        Err(Error::BackendUnavailable { .. })
    ));
    // With no other backend for the resource, the first unavailability is
    // reported.
    assert!(matches!(
        rm.open("GPIB0::1::INSTR"),
        Err(Error::BackendUnavailable {
            backend: "unavailable",
            ..
        })
    ));
}

#[test]
fn other_errors_stop_the_search() {
    // Nothing listens on this port: socket's NotFound is returned, not
    // hidden behind a later backend.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let rm = ResourceManager::builder()
        .no_default_backends()
        .backend(SocketBackend::new())
        .backend(Unavailable)
        .build()
        .unwrap();
    assert!(matches!(
        rm.open(&format!("TCPIP0::127.0.0.1::{port}::SOCKET")),
        Err(Error::NotFound(_))
    ));
}
