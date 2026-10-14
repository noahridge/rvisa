//! Control test and measurement instruments through VISA-style resources.
//!
//! A [`ResourceManager`] holds a list of [backends](Backend) in priority
//! order. [`ResourceManager::open`] parses the resource string and opens it
//! with the first backend that supports it, returning an [`Instrument`].
//! Code written against [`Session`] and [`SessionExt`] works the same
//! whichever backend opened it.
//!
//! ```
//! # #[cfg(feature = "mock")] {
//! use rvisa::prelude::*;
//!
//! // With the `mock` feature, the demo mock backend is registered.
//! let rm = ResourceManager::new()?;
//! let mut dmm = rm.open("TCPIP0::mock.local::5025::SOCKET")?;
//! dmm.set_read_termination(Some(b'\n'))?;
//! let idn = dmm.query("*IDN?")?;
//! assert!(idn.starts_with("ACME Instruments"));
//! # }
//! # Ok::<(), rvisa::Error>(())
//! ```
//!
//! # Backends
//!
//! In default priority order:
//!
//! | Name | Crate | Available | Supports |
//! |---|---|---|---|
//! | `mock` | `rvisa-mock` | feature `mock` | only its registered instruments |
//! | `vendor` | `rvisa-vendor` | feature `vendor` (default) | every VISA resource; needs a VISA installed |
//! | `socket` | `rvisa-socket` | feature `socket` (default) | `TCPIP::host::port::SOCKET`, in pure Rust |
//!
//! Vendor VISA comes before the pure-Rust backends until they have passed
//! conformance on hardware (ROADMAP §5); [`ResourceManagerBuilder::prefer`]
//! changes the order. Without a VISA installed, the vendor backend reports
//! [`Error::BackendUnavailable`] with instructions, and [`open`](ResourceManager::open)
//! moves on to the next backend that supports the resource, so socket
//! resources still open.
//!
//! # Features
//!
//! - `vendor` (default): the runtime-loaded vendor VISA backend.
//! - `socket` (default): the pure-Rust raw socket backend.
//! - `mock`: the scripted mock backend with demo instruments.
//! - `tokio`: [`tokio::AsyncInstrument`], an async adapter, and the socket
//!   backend's native async session (`socket::tokio`).

#![deny(missing_docs)]

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

pub use rvisa_core::*;
#[cfg(feature = "mock")]
pub use rvisa_mock as mock;
#[cfg(feature = "socket")]
pub use rvisa_socket as socket;
#[cfg(feature = "vendor")]
pub use rvisa_vendor as vendor;

#[cfg(feature = "tokio")]
pub mod tokio;

/// The traits and types most programs need: `use rvisa::prelude::*;`.
pub mod prelude {
    pub use crate::{Instrument, ResourceManager};
    pub use rvisa_core::{Session, SessionExt};
}

/// The facade's default write termination: the SCPI program message
/// terminator.
pub const DEFAULT_WRITE_TERMINATION: &[u8] = b"\n";

/// The backends compiled in, in default priority order.
///
/// The mock comes first: it supports only its own instruments, while the
/// vendor backend supports every VISA resource (DESIGN.md V10). Pure-Rust
/// backends follow vendor until Phase 6 (ROADMAP §5, DESIGN.md F3).
fn default_backends(
    #[allow(unused)] vendor_library: Option<&std::path::Path>,
) -> Vec<Arc<dyn Backend>> {
    vec![
        #[cfg(feature = "mock")]
        (Arc::new(rvisa_mock::MockBackend::demo()) as Arc<dyn Backend>),
        #[cfg(feature = "vendor")]
        (Arc::new(match vendor_library {
            Some(path) => rvisa_vendor::VendorBackend::with_library_path(path),
            None => rvisa_vendor::VendorBackend::new(),
        }) as Arc<dyn Backend>),
        #[cfg(feature = "socket")]
        (Arc::new(rvisa_socket::SocketBackend::new()) as Arc<dyn Backend>),
    ]
}

/// Opens instruments through a prioritised list of backends.
///
/// Instruments do not borrow the manager: dropping it leaves open
/// instruments working (ROADMAP D5).
#[derive(Clone)]
pub struct ResourceManager {
    backends: Vec<Arc<dyn Backend>>,
}

impl std::fmt::Debug for ResourceManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceManager")
            .field("backends", &self.backend_names())
            .finish()
    }
}

impl ResourceManager {
    /// A manager with every compiled-in backend, in default priority order.
    ///
    /// # Errors
    ///
    /// None at present; later backends may report setup failures here.
    pub fn new() -> Result<Self> {
        Self::builder().build()
    }

    /// Starts configuring a manager.
    pub fn builder() -> ResourceManagerBuilder {
        ResourceManagerBuilder {
            defaults: true,
            extra: Vec::new(),
            prefer: None,
            vendor_library: None,
        }
    }

    /// The backend names, in priority order.
    #[must_use]
    pub fn backend_names(&self) -> Vec<&'static str> {
        self.backends.iter().map(|b| b.name()).collect()
    }

    /// Opens `resource` with the first backend, in priority order, that
    /// supports it. A backend that is unavailable (no VISA installed) is
    /// skipped in favour of the next one that supports the resource
    /// (DESIGN.md F3); any other error is returned at once.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidResource`] if the name does not parse,
    /// [`Error::NoBackend`] if no backend supports it,
    /// [`Error::BackendUnavailable`] if every backend that supports it is
    /// unavailable, and errors from the backend.
    pub fn open(&self, resource: &str) -> Result<Instrument> {
        self.open_with_options(resource, None, &OpenOptions::new())
    }

    /// Opens `resource` with the named backend, bypassing the priority
    /// order.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open), and [`Error::InvalidArgument`] for an
    /// unknown backend name.
    pub fn open_with(&self, resource: &str, backend: &str) -> Result<Instrument> {
        self.open_with_options(resource, Some(backend), &OpenOptions::new())
    }

    /// Opens `resource` with `options`, using the named backend or, for
    /// `None`, the priority order.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open) and [`open_with`](Self::open_with).
    pub fn open_with_options(
        &self,
        resource: &str,
        backend: Option<&str>,
        options: &OpenOptions,
    ) -> Result<Instrument> {
        let rn = match resource.parse::<ResourceName>() {
            Ok(rn) => rn,
            Err(parse_error) => self.resolve_alias(resource)?.ok_or(parse_error)?,
        };
        let instrument = |backend: &Arc<dyn Backend>, session| Instrument {
            inner: session,
            backend: backend.name(),
            write_termination: Some(DEFAULT_WRITE_TERMINATION.to_vec()),
        };
        if let Some(name) = backend {
            let chosen = self.find(name)?;
            return Ok(instrument(chosen, chosen.open(&rn, options)?));
        }
        let mut unavailable = None;
        for candidate in self.backends.iter().filter(|b| b.supports(&rn)) {
            match candidate.open(&rn, options) {
                Ok(session) => return Ok(instrument(candidate, session)),
                Err(e @ Error::BackendUnavailable { .. }) => {
                    unavailable.get_or_insert(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(unavailable.unwrap_or(Error::NoBackend(rn)))
    }

    /// Lists resources matching a VISA search expression (such as `"?*"`
    /// or `"USB?*INSTR"`) across all backends, without duplicates, in
    /// priority order.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] for a malformed expression, and discovery
    /// errors from backends.
    pub fn list(&self, pattern: &str) -> Result<Vec<ResourceName>> {
        Pattern::new(pattern)?;
        let mut seen = HashSet::new();
        let mut names = Vec::new();
        let mut unavailable = None;
        let mut answered = false;
        for backend in &self.backends {
            match backend.list(pattern) {
                Ok(found) => {
                    answered = true;
                    for name in found {
                        if seen.insert(name.clone()) {
                            names.push(name);
                        }
                    }
                }
                // An unavailable backend (no VISA installed) does not hide
                // the others' results (DESIGN.md V3).
                Err(e @ Error::BackendUnavailable { .. }) => {
                    unavailable.get_or_insert(e);
                }
                Err(e) => return Err(e),
            }
        }
        match unavailable {
            Some(e) if !answered => Err(e),
            _ => Ok(names),
        }
    }

    /// Asks each backend, in priority order, to resolve a name that is not a
    /// resource string (DESIGN.md V9).
    fn resolve_alias(&self, alias: &str) -> Result<Option<ResourceName>> {
        for backend in &self.backends {
            match backend.resolve_alias(alias) {
                Ok(Some(rn)) => return Ok(Some(rn)),
                Ok(None) | Err(Error::BackendUnavailable { .. }) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    fn find(&self, name: &str) -> Result<&Arc<dyn Backend>> {
        self.backends
            .iter()
            .find(|b| b.name() == name)
            .ok_or_else(|| Error::InvalidArgument(format!("no backend named {name:?}")))
    }
}

/// Configures a [`ResourceManager`].
#[must_use]
pub struct ResourceManagerBuilder {
    defaults: bool,
    extra: Vec<Arc<dyn Backend>>,
    prefer: Option<Vec<String>>,
    vendor_library: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for ResourceManagerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceManagerBuilder")
            .field("defaults", &self.defaults)
            .field(
                "extra",
                &self.extra.iter().map(|b| b.name()).collect::<Vec<_>>(),
            )
            .field("prefer", &self.prefer)
            .finish()
    }
}

impl ResourceManagerBuilder {
    /// Adds a backend after the defaults. A backend with the same name as
    /// one already registered replaces it.
    pub fn backend(mut self, backend: impl Backend + 'static) -> Self {
        self.extra.push(Arc::new(backend));
        self
    }

    /// Adds a shared backend; see [`backend`](Self::backend).
    pub fn backend_arc(mut self, backend: Arc<dyn Backend>) -> Self {
        self.extra.push(backend);
        self
    }

    /// Tries the VISA library at `path` before the default locations
    /// (ROADMAP D2).
    #[cfg(feature = "vendor")]
    pub fn vendor_library(mut self, path: impl AsRef<std::path::Path>) -> Self {
        self.vendor_library = Some(path.as_ref().to_path_buf());
        self
    }

    /// Leaves out the compiled-in backends.
    pub fn no_default_backends(mut self) -> Self {
        self.defaults = false;
        self
    }

    /// Puts the named backends first, in this order. The others follow in
    /// registration order.
    pub fn prefer(mut self, names: &[&str]) -> Self {
        self.prefer = Some(names.iter().map(|&n| n.to_owned()).collect());
        self
    }

    /// Builds the manager.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if [`prefer`](Self::prefer) names a
    /// backend that is not registered.
    pub fn build(self) -> Result<ResourceManager> {
        let mut backends: Vec<Arc<dyn Backend>> = Vec::new();
        let registered = if self.defaults {
            default_backends(self.vendor_library.as_deref())
        } else {
            Vec::new()
        };
        for backend in registered.into_iter().chain(self.extra) {
            match backends.iter().position(|b| b.name() == backend.name()) {
                Some(i) => backends[i] = backend,
                None => backends.push(backend),
            }
        }
        if let Some(prefer) = self.prefer {
            let mut ordered = Vec::with_capacity(backends.len());
            for name in &prefer {
                let i = backends
                    .iter()
                    .position(|b| b.name() == name)
                    .ok_or_else(|| {
                        Error::InvalidArgument(format!("prefer: no backend named {name:?}"))
                    })?;
                ordered.push(backends.remove(i));
            }
            ordered.append(&mut backends);
            backends = ordered;
        }
        Ok(ResourceManager { backends })
    }
}

/// An open instrument: a backend session plus the facade's write
/// termination (ROADMAP D9).
///
/// [`SessionExt::query`] and [`SessionExt::write_str`] append the write
/// termination (`\n` by default); [`Session::write`] sends bytes unchanged.
pub struct Instrument {
    inner: Box<dyn Session>,
    backend: &'static str,
    write_termination: Option<Vec<u8>>,
}

impl std::fmt::Debug for Instrument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Instrument")
            .field("resource", self.inner.resource())
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

impl Instrument {
    /// The name of the backend that opened this instrument.
    #[must_use]
    pub fn backend(&self) -> &'static str {
        self.backend
    }

    /// The backend session, without the facade's write termination.
    #[must_use]
    pub fn into_inner(self) -> Box<dyn Session> {
        self.inner
    }
}

impl Session for Instrument {
    fn resource(&self) -> &ResourceName {
        self.inner.resource()
    }
    fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.inner.write(data)
    }
    fn read(&mut self, max: usize) -> Result<ReadResult> {
        self.inner.read(max)
    }
    fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<()> {
        self.inner.set_timeout(timeout)
    }
    fn timeout(&self) -> Option<Duration> {
        self.inner.timeout()
    }
    fn set_read_termination(&mut self, term: Option<u8>) -> Result<()> {
        self.inner.set_read_termination(term)
    }
    fn write_termination(&self) -> Option<&[u8]> {
        self.write_termination.as_deref()
    }
    fn set_write_termination(&mut self, term: Option<&[u8]>) -> Result<()> {
        self.write_termination = term.map(<[u8]>::to_vec);
        Ok(())
    }
    fn clear(&mut self) -> Result<()> {
        self.inner.clear()
    }
    fn read_stb(&mut self) -> Result<u8> {
        self.inner.read_stb()
    }
    fn assert_trigger(&mut self) -> Result<()> {
        self.inner.assert_trigger()
    }
    fn lock(&mut self, kind: LockKind, timeout: Duration) -> Result<()> {
        self.inner.lock(kind, timeout)
    }
    fn unlock(&mut self) -> Result<()> {
        self.inner.unlock()
    }
    fn get_attr(&self, attr: Attr) -> Result<AttrValue> {
        self.inner.get_attr(attr)
    }
    fn set_attr(&mut self, attr: Attr, value: AttrValue) -> Result<()> {
        self.inner.set_attr(attr, value)
    }
    fn enable_event(&mut self, kind: EventKind) -> Result<()> {
        self.inner.enable_event(kind)
    }
    fn wait_event(&mut self, kind: EventKind, timeout: Duration) -> Result<Event> {
        self.inner.wait_event(kind, timeout)
    }
    fn disable_event(&mut self, kind: EventKind) -> Result<()> {
        self.inner.disable_event(kind)
    }
}
