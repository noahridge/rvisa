//! Backend selection, listing and write termination.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use rvisa::{
    Backend, Error, Instrument, OpenOptions, ResourceManager, ResourceName, Result, Session,
    SessionExt,
};
use rvisa_mock::{MockBackend, MockInstrument, Script};

/// A mock backend under another name, to test priority between backends.
struct Named(&'static str, MockBackend);

impl Backend for Named {
    fn name(&self) -> &'static str {
        self.0
    }
    fn supports(&self, rn: &ResourceName) -> bool {
        self.1.supports(rn)
    }
    fn open(&self, rn: &ResourceName, options: &OpenOptions) -> Result<Box<dyn Session>> {
        self.1.open(rn, options)
    }
    fn list(&self, pattern: &str) -> Result<Vec<ResourceName>> {
        self.1.list(pattern)
    }
}

fn rn(s: &str) -> ResourceName {
    s.parse().unwrap()
}

fn echo(answer: &str) -> Script {
    Script::new()
        .on_write("*IDN?\n")
        .respond(format!("{answer}\n"))
        .on_write("*IDN?\r\n")
        .respond(format!("{answer} crlf\n"))
}

/// Two backends that both have `GPIB0::1`, plus one instrument each.
fn two_backends() -> (ResourceManager, MockInstrument) {
    let first = MockBackend::new()
        .instrument(rn("GPIB0::1"), echo("first"))
        .instrument(rn("GPIB0::2"), echo("only-first"));
    let second = MockBackend::new()
        .instrument(rn("GPIB0::1"), echo("second"))
        .instrument(rn("GPIB0::3"), echo("only-second"));
    let handle = first.handle(&rn("GPIB0::1")).unwrap();
    let rm = ResourceManager::builder()
        .no_default_backends()
        .backend(Named("first", first))
        .backend(Named("second", second))
        .build()
        .unwrap();
    (rm, handle)
}

fn idn(instrument: &mut Instrument) -> String {
    instrument.query("*IDN?").unwrap()
}

#[test]
fn priority_and_forced_backend() {
    let (rm, _) = two_backends();
    assert_eq!(rm.backend_names(), ["first", "second"]);

    let mut i = rm.open("GPIB0::1::INSTR").unwrap();
    assert_eq!((i.backend(), idn(&mut i).as_str()), ("first", "first"));
    // Falls through to the first backend that supports the resource.
    let mut i = rm.open("gpib0::3").unwrap();
    assert_eq!(
        (i.backend(), idn(&mut i).as_str()),
        ("second", "only-second")
    );

    let mut i = rm.open_with("GPIB0::1", "second").unwrap();
    assert_eq!(idn(&mut i), "second");
    assert!(matches!(
        rm.open_with("GPIB0::2", "second"),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        rm.open_with("GPIB0::1", "vendor"),
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(rm.open("GPIB0::9"), Err(Error::NoBackend(_))));
    assert!(matches!(rm.open("bogus"), Err(Error::InvalidResource(_))));
}

#[test]
fn prefer_reorders() {
    let first = MockBackend::new().instrument(rn("GPIB0::1"), echo("first"));
    let second = MockBackend::new().instrument(rn("GPIB0::1"), echo("second"));
    let rm = ResourceManager::builder()
        .no_default_backends()
        .backend(Named("first", first))
        .backend(Named("second", second))
        .prefer(&["second"])
        .build()
        .unwrap();
    assert_eq!(rm.backend_names(), ["second", "first"]);
    let mut i = rm.open("GPIB0::1").unwrap();
    assert_eq!(idn(&mut i), "second");

    let err = ResourceManager::builder()
        .no_default_backends()
        .prefer(&["vendor"])
        .build()
        .unwrap_err();
    assert!(matches!(err, Error::InvalidArgument(_)));
}

#[test]
fn same_name_replaces() {
    let rm = ResourceManager::builder()
        .no_default_backends()
        .backend(Named("x", MockBackend::new()))
        .backend(Named(
            "x",
            MockBackend::new().instrument(rn("ASRL1"), echo("replaced")),
        ))
        .build()
        .unwrap();
    assert_eq!(rm.backend_names(), ["x"]);
    assert_eq!(idn(&mut rm.open("ASRL1").unwrap()), "replaced");
}

#[test]
fn list_merges_without_duplicates() {
    let (rm, _) = two_backends();
    assert_eq!(
        rm.list("?*").unwrap(),
        [rn("GPIB0::1"), rn("GPIB0::2"), rn("GPIB0::3")]
    );
    assert_eq!(rm.list("GPIB0::3?*").unwrap(), [rn("GPIB0::3")]);
    assert!(rm.list("(").is_err());
}

#[test]
fn write_termination_is_appended_by_the_facade() {
    let (rm, handle) = two_backends();
    let mut i = rm.open("GPIB0::1").unwrap();
    assert_eq!(i.write_termination(), Some(&b"\n"[..]));

    assert_eq!(i.query("*IDN?").unwrap(), "first");
    i.set_write_termination(Some(b"\r\n")).unwrap();
    assert_eq!(i.query("*IDN?").unwrap(), "first crlf");
    i.set_write_termination(None).unwrap();
    i.write_str("RAW").unwrap();
    // `write` never appends.
    i.write(b"BYTES").unwrap();
    assert_eq!(
        handle.writes(),
        [
            b"*IDN?\n".to_vec(),
            b"*IDN?\r\n".to_vec(),
            b"RAW".to_vec(),
            b"BYTES".to_vec()
        ]
    );

    // Backend sessions have no write termination of their own (D9).
    let mut raw = i.into_inner();
    assert_eq!(raw.write_termination(), None);
    assert!(matches!(
        raw.set_write_termination(Some(b"\n")),
        Err(Error::Unsupported { .. })
    ));
}

#[test]
fn instruments_outlive_the_manager() {
    let (rm, _) = two_backends();
    let mut i = rm.open("GPIB0::1").unwrap();
    drop(rm);
    assert_eq!(idn(&mut i), "first");
    assert_eq!(i.resource(), &rn("GPIB0::1::INSTR"));
    assert!(format!("{i:?}").contains("first"));
}

#[test]
fn forwards_every_session_method() {
    let backend = MockBackend::demo();
    let rm = ResourceManager::builder()
        .no_default_backends()
        .backend(backend)
        .build()
        .unwrap();
    let mut i = rm.open("ASRL1::INSTR").unwrap();
    let report = rvisa_conformance::run_all(
        || {
            rm.open("ASRL1::INSTR")
                .map(|i| Box::new(i) as Box<dyn Session>)
        },
        &rvisa_conformance::Profile::scpi()
            .multi_line_command(b"MULTI?\n")
            .binary_block_command(b"DATA?\n", Some(8))
            .large_block_command(b"BLOCK:LARGE?\n")
            .srq_command(b"SRQ\n"),
    );
    report.assert_all_passed();
    let block = i.query_ieee488_block("DATA?").unwrap();
    assert_eq!(block.len(), 8);
}

#[test]
fn default_backends_follow_features() {
    let rm = ResourceManager::new().unwrap();
    let mut expected = Vec::new();
    if cfg!(feature = "mock") {
        expected.push("mock");
    }
    if cfg!(feature = "vendor") {
        expected.push("vendor");
    }
    assert_eq!(rm.backend_names(), expected);
    if expected.is_empty() {
        assert!(matches!(rm.open("GPIB0::1"), Err(Error::NoBackend(_))));
    }
}

#[cfg(feature = "mock")]
#[test]
fn mock_comes_before_vendor() {
    let rm = ResourceManager::new().unwrap();
    let i = rm.open("TCPIP0::mock.local::INSTR").unwrap();
    assert_eq!(i.backend(), "mock");
}
