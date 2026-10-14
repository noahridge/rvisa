//! An async adapter for tokio (feature `tokio`, ROADMAP D8).
//!
//! Sessions are blocking, so each call runs on tokio's blocking thread pool
//! with [`spawn_blocking`](::tokio::task::spawn_blocking). The instrument is
//! held in an `Arc<Mutex<_>>`: if a future is dropped, the blocking call
//! still finishes and releases the instrument, so it is never lost or left
//! half-used (DESIGN.md V11). Pure-Rust backends will offer native async
//! later.
//!
//! ```no_run
//! # async fn demo() -> rvisa::Result<()> {
//! use rvisa::tokio::AsyncResourceManager;
//!
//! let rm = AsyncResourceManager::new(rvisa::ResourceManager::new()?);
//! let dmm = rm.open("TCPIP0::192.168.1.50::INSTR").await?;
//! let idn = dmm.query("*IDN?").await?;
//! # Ok(()) }
//! ```

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::{
    Error, Event, EventKind, Instrument, ReadResult, ResourceManager, ResourceName, Result,
    Session, SessionExt,
};

/// Runs `f` on the blocking pool, mapping a failed join (a panic in `f`)
/// to [`Error::Io`].
async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    ::tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))?
}

/// A [`ResourceManager`] whose `open` and `list` run on the blocking pool.
#[derive(Debug, Clone)]
pub struct AsyncResourceManager {
    rm: ResourceManager,
}

impl AsyncResourceManager {
    /// Wraps a resource manager.
    #[must_use]
    pub fn new(rm: ResourceManager) -> Self {
        Self { rm }
    }

    /// See [`ResourceManager::open`].
    ///
    /// # Errors
    ///
    /// As [`ResourceManager::open`].
    pub async fn open(&self, resource: &str) -> Result<AsyncInstrument> {
        let (rm, resource) = (self.rm.clone(), resource.to_owned());
        blocking(move || rm.open(&resource))
            .await
            .map(AsyncInstrument::new)
    }

    /// See [`ResourceManager::list`].
    ///
    /// # Errors
    ///
    /// As [`ResourceManager::list`].
    pub async fn list(&self, pattern: &str) -> Result<Vec<ResourceName>> {
        let (rm, pattern) = (self.rm.clone(), pattern.to_owned());
        blocking(move || rm.list(&pattern)).await
    }
}

/// An [`Instrument`] used from async code. Cloning shares the instrument;
/// calls run one at a time.
#[derive(Debug, Clone)]
pub struct AsyncInstrument {
    inner: Arc<Mutex<Instrument>>,
}

impl AsyncInstrument {
    /// Wraps an instrument.
    #[must_use]
    pub fn new(instrument: Instrument) -> Self {
        Self {
            inner: Arc::new(Mutex::new(instrument)),
        }
    }

    /// Runs `f` with the instrument on the blocking pool. Use this for
    /// anything the convenience methods do not cover.
    ///
    /// # Errors
    ///
    /// Whatever `f` returns, or [`Error::Io`] if it panics.
    pub async fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Instrument) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        let inner = Arc::clone(&self.inner);
        blocking(move || {
            let mut instrument = inner.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut instrument)
        })
        .await
    }

    /// The instrument, if no other clone or call holds it.
    ///
    /// A call whose future was dropped keeps running on the blocking pool,
    /// and holds the instrument until it finishes. This can return `None`
    /// for a moment after such a call has released the lock.
    #[must_use]
    pub fn into_inner(self) -> Option<Instrument> {
        Arc::into_inner(self.inner).map(|m| m.into_inner().unwrap_or_else(PoisonError::into_inner))
    }

    /// See [`Session::write`].
    ///
    /// # Errors
    ///
    /// As [`Session::write`].
    pub async fn write(&self, data: impl Into<Vec<u8>>) -> Result<usize> {
        let data = data.into();
        self.run(move |i| i.write(&data)).await
    }

    /// See [`Session::read`].
    ///
    /// # Errors
    ///
    /// As [`Session::read`].
    pub async fn read(&self, max: usize) -> Result<ReadResult> {
        self.run(move |i| i.read(max)).await
    }

    /// See [`SessionExt::write_str`].
    ///
    /// # Errors
    ///
    /// As [`SessionExt::write_str`].
    pub async fn write_str(&self, command: impl Into<String>) -> Result<()> {
        let command = command.into();
        self.run(move |i| i.write_str(&command)).await
    }

    /// See [`SessionExt::query`].
    ///
    /// # Errors
    ///
    /// As [`SessionExt::query`].
    pub async fn query(&self, command: impl Into<String>) -> Result<String> {
        let command = command.into();
        self.run(move |i| i.query(&command)).await
    }

    /// See [`SessionExt::query_bytes`].
    ///
    /// # Errors
    ///
    /// As [`SessionExt::query_bytes`].
    pub async fn query_bytes(&self, command: impl Into<Vec<u8>>) -> Result<Vec<u8>> {
        let command = command.into();
        self.run(move |i| i.query_bytes(&command)).await
    }

    /// See [`SessionExt::read_ieee488_block`].
    ///
    /// # Errors
    ///
    /// As [`SessionExt::read_ieee488_block`].
    pub async fn read_ieee488_block(&self) -> Result<Vec<u8>> {
        self.run(SessionExt::read_ieee488_block).await
    }

    /// See [`SessionExt::query_ieee488_block`].
    ///
    /// # Errors
    ///
    /// As [`SessionExt::query_ieee488_block`].
    pub async fn query_ieee488_block(&self, command: impl Into<String>) -> Result<Vec<u8>> {
        let command = command.into();
        self.run(move |i| i.query_ieee488_block(&command)).await
    }

    /// See [`Session::set_timeout`].
    ///
    /// # Errors
    ///
    /// As [`Session::set_timeout`].
    pub async fn set_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        self.run(move |i| i.set_timeout(timeout)).await
    }

    /// See [`Session::set_read_termination`].
    ///
    /// # Errors
    ///
    /// As [`Session::set_read_termination`].
    pub async fn set_read_termination(&self, term: Option<u8>) -> Result<()> {
        self.run(move |i| i.set_read_termination(term)).await
    }

    /// See [`Session::clear`].
    ///
    /// # Errors
    ///
    /// As [`Session::clear`].
    pub async fn clear(&self) -> Result<()> {
        self.run(Session::clear).await
    }

    /// See [`Session::read_stb`].
    ///
    /// # Errors
    ///
    /// As [`Session::read_stb`].
    pub async fn read_stb(&self) -> Result<u8> {
        self.run(Session::read_stb).await
    }

    /// See [`Session::assert_trigger`].
    ///
    /// # Errors
    ///
    /// As [`Session::assert_trigger`].
    pub async fn assert_trigger(&self) -> Result<()> {
        self.run(Session::assert_trigger).await
    }

    /// See [`Session::enable_event`].
    ///
    /// # Errors
    ///
    /// As [`Session::enable_event`].
    pub async fn enable_event(&self, kind: EventKind) -> Result<()> {
        self.run(move |i| i.enable_event(kind)).await
    }

    /// Waits for an event without blocking the async runtime. The
    /// instrument is busy (other calls wait) until the event arrives or
    /// `timeout` passes.
    ///
    /// # Errors
    ///
    /// As [`Session::wait_event`].
    pub async fn wait_event(&self, kind: EventKind, timeout: Duration) -> Result<Event> {
        self.run(move |i| i.wait_event(kind, timeout)).await
    }

    /// See [`Session::disable_event`].
    ///
    /// # Errors
    ///
    /// As [`Session::disable_event`].
    pub async fn disable_event(&self, kind: EventKind) -> Result<()> {
        self.run(move |i| i.disable_event(kind)).await
    }
}
