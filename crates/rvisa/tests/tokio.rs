//! The async adapter.

#![cfg(feature = "tokio")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use rvisa::tokio::AsyncResourceManager;
use rvisa::{Error, EventKind, ResourceManager};
use rvisa_mock::MockBackend;

fn rm() -> AsyncResourceManager {
    AsyncResourceManager::new(
        ResourceManager::builder()
            .no_default_backends()
            .backend(MockBackend::demo())
            .build()
            .unwrap(),
    )
}

#[tokio::test]
async fn queries_run_on_the_blocking_pool() {
    let rm = rm();
    assert_eq!(rm.list("GPIB?*").await.unwrap().len(), 1);
    let psu = rm.open("GPIB0::22::INSTR").await.unwrap();
    assert!(psu.query("*IDN?").await.unwrap().contains("PSU30"));
    assert_eq!(psu.query_ieee488_block("DATA?").await.unwrap().len(), 8);
    psu.write_str("*CLS").await.unwrap();
    assert_eq!(psu.write(b"*OPC?\n".to_vec()).await.unwrap(), 6);
    assert_eq!(psu.read(64).await.unwrap().data, b"1\n");
    psu.set_timeout(Some(Duration::from_millis(50)))
        .await
        .unwrap();
    assert!(matches!(psu.read(64).await, Err(Error::Timeout { .. })));
    psu.set_read_termination(Some(b'\n')).await.unwrap();
    psu.clear().await.unwrap();
    psu.assert_trigger().await.unwrap();

    psu.enable_event(EventKind::ServiceRequest).await.unwrap();
    psu.write_str("SRQ").await.unwrap();
    psu.wait_event(EventKind::ServiceRequest, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(psu.read_stb().await.unwrap() & 0x40, 0x40);
    psu.disable_event(EventKind::ServiceRequest).await.unwrap();
    assert!(psu.into_inner().is_some());
}

#[tokio::test]
async fn a_cancelled_call_does_not_lose_the_instrument() {
    let rm = rm();
    let dmm = rm.open("TCPIP0::mock.local::INSTR").await.unwrap();
    dmm.enable_event(EventKind::ServiceRequest).await.unwrap();
    // Cancel a wait that would take 300 ms.
    let wait = dmm.wait_event(EventKind::ServiceRequest, Duration::from_millis(300));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), wait)
            .await
            .is_err()
    );
    // The blocking wait finishes, then the instrument is usable again.
    assert!(dmm.query("*IDN?").await.unwrap().starts_with("ACME"));
}

#[tokio::test]
async fn into_inner_needs_the_last_handle() {
    // A fresh instrument: a cancelled call may still hold a handle for a
    // moment after it releases the lock, which would make this racy.
    let rm = rm();
    let psu = rm.open("GPIB0::22::INSTR").await.unwrap();
    assert!(psu.query("*IDN?").await.unwrap().contains("PSU30"));
    let clone = psu.clone();
    assert!(psu.into_inner().is_none(), "a clone still holds it");
    assert!(clone.into_inner().is_some());
    let unknown = rm.open("GPIB0::29").await;
    assert!(matches!(unknown, Err(Error::NoBackend(_))), "{unknown:?}");
}
