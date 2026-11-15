//! The facade with the VXI-11 backend.

#![cfg(feature = "vxi11")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rvisa::prelude::*;
use rvisa::vxi11::{Vxi11Backend, Vxi11Server};
use rvisa::{Backend, OpenOptions, ResourceName};
use rvisa_mock::{MockBackend, demo_script};

#[test]
fn vxi11_is_registered_by_default() {
    let rm = ResourceManager::new().unwrap();
    assert!(rm.backend_names().contains(&"vxi11"));
}

#[test]
fn instr_resources_open_with_vxi11() {
    let rn: ResourceName = "TCPIP0::sim::INSTR".parse().unwrap();
    let mock = MockBackend::new().instrument(rn.clone(), demo_script("SIM100", "SIM001"));
    let server = Vxi11Server::spawn(move |_| mock.open(&rn, &OpenOptions::new())).unwrap();
    let rm = ResourceManager::builder()
        .no_default_backends()
        .backend(Vxi11Backend::new().portmapper_port(server.port()))
        .build()
        .unwrap();
    let mut dmm = rm.open("TCPIP::127.0.0.1::INSTR").unwrap();
    assert_eq!(dmm.backend(), "vxi11");
    // No term char needed: VXI-11 has END.
    assert!(
        dmm.query("*IDN?")
            .unwrap()
            .starts_with("ACME Instruments,SIM100")
    );
    assert!(matches!(
        rm.open("TCPIP0::127.0.0.1::hislip0::INSTR"),
        Err(rvisa::Error::NoBackend(_))
    ));
}
