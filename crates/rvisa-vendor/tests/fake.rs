//! The vendor backend against the fake VISA library: real `dlopen`, real C
//! calls, mock instruments behind them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{DEMO, demo_profile, fake_library, serial};
use rvisa_conformance::run_all;
use rvisa_core::{
    Attr, AttrValue, Backend, Error, LockKind, OpenOptions, ResourceName, Session, SessionExt,
};
use rvisa_vendor::{RenMode, VendorBackend, visa};

fn rn(s: &str) -> ResourceName {
    s.parse().unwrap()
}

fn backend() -> VendorBackend {
    VendorBackend::with_library_path(fake_library())
}

#[test]
fn every_demo_instrument_conforms() {
    let _serial = serial();
    let visa = backend();
    for name in DEMO {
        let resource = rn(name);
        let report = run_all(
            || visa.open(&resource, &OpenOptions::new()),
            &demo_profile(),
        );
        println!("{name}:\n{report}");
        report.assert_all_passed();
    }
}

#[test]
fn loads_the_fake() {
    let _serial = serial();
    let visa = backend();
    visa.load().unwrap();
    assert!(visa.library_path().unwrap().contains("rvisa_vendor_fake"));
    assert_eq!(visa.missing_functions().unwrap(), Vec::<&str>::new());
    assert_eq!(visa.name(), "vendor");
}

#[test]
fn discovery_and_aliases() {
    let _serial = serial();
    let visa = backend();
    let all = visa.list("?*").unwrap();
    assert_eq!(all.len(), 5);
    assert_eq!(visa.list("USB?*INSTR").unwrap(), [rn(DEMO[2])]);
    assert!(visa.list("VXI?*").unwrap().is_empty());
    assert!(matches!(visa.list("["), Err(Error::InvalidArgument(_))));

    assert_eq!(visa.resolve_alias("mockdmm").unwrap(), Some(rn(DEMO[0])));
    assert_eq!(visa.resolve_alias("nosuchalias").unwrap(), None);
}

#[test]
fn error_mapping() {
    let _serial = serial();
    let visa = backend();
    assert!(matches!(
        visa.open(&rn("GPIB0::9::INSTR"), &OpenOptions::new()),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        visa.open(&rn("/dev/ttyUSB0"), &OpenOptions::new()),
        Err(Error::Unsupported { .. })
    ));

    let mut s = visa.open(&rn(DEMO[3]), &OpenOptions::new()).unwrap();
    s.set_timeout(Some(Duration::from_millis(20))).unwrap();
    assert!(matches!(s.read(16), Err(Error::Timeout)));
    assert!(matches!(s.read(0), Err(Error::InvalidArgument(_))));
    assert!(matches!(
        s.set_attr(Attr::ModelName, AttrValue::Str("x".into())),
        Err(Error::AttrReadOnly(Attr::ModelName))
    ));
    assert!(matches!(
        s.get_attr(Attr::UsbSerialNumber),
        Err(Error::Unsupported { .. })
    ));
    assert!(matches!(s.unlock(), Err(Error::InvalidArgument(_))));
}

#[test]
fn locks_between_sessions() {
    let _serial = serial();
    let visa = backend();
    let mut a = visa.open(&rn(DEMO[4]), &OpenOptions::new()).unwrap();
    let mut b = visa.open(&rn(DEMO[4]), &OpenOptions::new()).unwrap();
    let short = Duration::from_millis(20);
    a.lock(LockKind::Exclusive, short).unwrap();
    assert!(matches!(b.write(b"*IDN?\n"), Err(Error::ResourceLocked)));
    assert!(matches!(
        b.lock(LockKind::Exclusive, short),
        Err(Error::ResourceLocked)
    ));
    a.unlock().unwrap();
    b.lock(LockKind::Shared { key: "k".into() }, short).unwrap();
    a.lock(LockKind::Shared { key: "k".into() }, short).unwrap();
    assert!(a.query_bytes(b"*IDN?\n").is_ok());
    a.unlock().unwrap();
    b.unlock().unwrap();

    let options = OpenOptions::new()
        .lock(LockKind::Exclusive)
        .open_timeout(short);
    let _held = visa.open(&rn(DEMO[4]), &options).unwrap();
    assert!(matches!(
        visa.open(&rn(DEMO[4]), &options),
        Err(Error::ResourceLocked)
    ));
}

#[test]
fn attributes() {
    let _serial = serial();
    let visa = backend();
    let mut usb = visa.open(&rn(DEMO[2]), &OpenOptions::new()).unwrap();
    assert_eq!(
        usb.get_attr(Attr::UsbSerialNumber).unwrap(),
        AttrValue::Str("MOCK001".into())
    );
    assert_eq!(
        usb.get_attr(Attr::ManufacturerName).unwrap(),
        AttrValue::Str("ACME Instruments".into())
    );
    assert_eq!(usb.timeout(), Some(Duration::from_secs(2)));

    // Raw access by VISA ID.
    assert_eq!(
        usb.get_attr(Attr::Raw(visa::VI_ATTR_TMO_VALUE)).unwrap(),
        AttrValue::U64(2000)
    );
    assert_eq!(
        usb.get_attr(Attr::Raw(visa::VI_ATTR_RSRC_NAME)).unwrap(),
        AttrValue::Str(DEMO[2].into())
    );
    usb.set_attr(Attr::Raw(visa::VI_ATTR_TMO_VALUE), AttrValue::U64(1500))
        .unwrap();
    assert_eq!(
        usb.get_attr(Attr::TimeoutValue).unwrap(),
        AttrValue::Timeout(Some(Duration::from_millis(1500)))
    );
    assert!(matches!(
        usb.get_attr(Attr::Raw(0x3FFF_0FFF)),
        Err(Error::Unsupported { .. })
    ));

    usb.set_timeout(None).unwrap();
    assert_eq!(usb.timeout(), None);
    assert_eq!(
        usb.get_attr(Attr::TimeoutValue).unwrap(),
        AttrValue::Timeout(None)
    );

    let mut serial_port = visa.open(&rn(DEMO[4]), &OpenOptions::new()).unwrap();
    for (attr, value) in [
        (Attr::AsrlBaud, AttrValue::U32(115_200)),
        (Attr::AsrlDataBits, AttrValue::U16(7)),
        (Attr::AsrlParity, AttrValue::Parity(rvisa_core::Parity::Odd)),
        (
            Attr::AsrlStopBits,
            AttrValue::StopBits(rvisa_core::StopBits::Two),
        ),
        (
            Attr::AsrlFlowControl,
            AttrValue::FlowControl(rvisa_core::FlowControl::XON_XOFF),
        ),
    ] {
        serial_port.set_attr(attr, value.clone()).unwrap();
        assert_eq!(serial_port.get_attr(attr).unwrap(), value, "{attr:?}");
    }
    assert_eq!(
        serial_port.get_attr(Attr::InterfaceType).unwrap(),
        AttrValue::InterfaceType(rvisa_core::InterfaceType::Asrl)
    );
}

#[test]
fn vendor_only_operations() {
    let _serial = serial();
    let visa = backend();
    let mut gpib = visa.open_vendor(&rn(DEMO[3]), &OpenOptions::new()).unwrap();
    gpib.gpib_control_ren(RenMode::AssertLocalLockout).unwrap();
    gpib.flush(visa::VI_READ_BUF_DISCARD | visa::VI_WRITE_BUF_DISCARD)
        .unwrap();
    let mut tcpip = visa.open_vendor(&rn(DEMO[0]), &OpenOptions::new()).unwrap();
    assert!(matches!(
        tcpip.gpib_control_ren(RenMode::Assert),
        Err(Error::Unsupported { .. })
    ));
    assert!(matches!(tcpip.flush(0), Err(Error::Vendor { .. })));
}

#[test]
fn sessions_outlive_the_backend() {
    let _serial = serial();
    let visa = backend();
    let mut s = visa.open(&rn(DEMO[0]), &OpenOptions::new()).unwrap();
    drop(visa);
    assert!(s.query_bytes(b"*IDN?\n").unwrap().starts_with(b"ACME"));
}

#[test]
fn missing_library_is_backend_unavailable() {
    let visa = VendorBackend::with_library_path("/nonexistent/libvisa");
    // Construction and `supports` never touch the library.
    assert!(visa.supports(&rn("TCPIP0::10.0.0.1::INSTR")));
    assert!(visa.supports(&rn("USB0::1::2::s")));
    assert!(!visa.supports(&rn("/dev/ttyUSB0")));
    // Without a VISA installed on the test machine, the defaults fail too
    // and every operation explains how to fix it. With one installed, this
    // part is skipped.
    if VendorBackend::with_library_path("/nonexistent/libvisa")
        .load()
        .is_ok()
    {
        return;
    }
    for err in [
        visa.open(&rn("GPIB0::1"), &OpenOptions::new())
            .map(drop)
            .unwrap_err(),
        visa.list("?*").unwrap_err(),
        visa.resolve_alias("dmm").unwrap_err(),
        visa.load().unwrap_err(),
    ] {
        match err {
            Error::BackendUnavailable { backend, reason } => {
                assert_eq!(backend, "vendor");
                assert!(reason.contains("/nonexistent/libvisa"), "{reason}");
                assert!(reason.contains("RVISA_VENDOR_LIB"), "{reason}");
            }
            other => panic!("expected BackendUnavailable, got {other:?}"),
        }
    }
}
