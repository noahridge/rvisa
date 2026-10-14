//! `VendorBackend`: lazy loading, discovery and opening sessions.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use rvisa_core::{Backend, Error, OpenOptions, ResourceName, Result, Session};

use crate::BACKEND;
use crate::convert::{self, Context};
use crate::ffi::{Library, c_string};
use crate::session::VendorSession;
use crate::visa::*;

/// The default resource manager session, shared by every session opened
/// from it and closed when the last one is dropped (ROADMAP D5).
#[derive(Debug)]
pub(crate) struct Rm {
    pub(crate) lib: Library,
    pub(crate) vi: ViSession,
}

impl Drop for Rm {
    fn drop(&mut self) {
        self.lib.close(self.vi);
    }
}

/// The vendor VISA backend (`"vendor"`): NI-VISA, Keysight VISA, R&S VISA
/// or any other VPP-4.3 implementation, loaded at runtime.
///
/// Construction never fails. The library is loaded on first use; if none
/// can be loaded, operations return [`Error::BackendUnavailable`] with the
/// paths tried and how to fix it (DESIGN.md V3).
///
/// ```no_run
/// use rvisa_core::{Backend, OpenOptions, SessionExt};
/// use rvisa_vendor::VendorBackend;
///
/// let visa = VendorBackend::new();
/// for name in visa.list("?*INSTR")? {
///     println!("{name}");
/// }
/// let mut dmm = visa.open(&"TCPIP0::192.168.1.50::INSTR".parse()?, &OpenOptions::new())?;
/// println!("{}", String::from_utf8_lossy(&dmm.query_bytes(b"*IDN?\n")?));
/// # Ok::<(), rvisa_core::Error>(())
/// ```
#[derive(Debug, Default)]
pub struct VendorBackend {
    path: Option<PathBuf>,
    rm: OnceLock<std::result::Result<Arc<Rm>, String>>,
}

impl VendorBackend {
    /// A backend that searches the default locations (and the
    /// `RVISA_VENDOR_LIB` variable) when first used.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A backend that tries `path` before the default locations.
    #[must_use]
    pub fn with_library_path(path: impl AsRef<Path>) -> Self {
        Self {
            path: Some(path.as_ref().to_path_buf()),
            rm: OnceLock::new(),
        }
    }

    /// Loads the library now, instead of on first use.
    ///
    /// # Errors
    ///
    /// [`Error::BackendUnavailable`] if no VISA library could be loaded or
    /// its resource manager could not be opened.
    pub fn load(&self) -> Result<()> {
        self.rm().map(drop)
    }

    /// The path of the loaded library, loading it if needed.
    ///
    /// # Errors
    ///
    /// As [`load`](Self::load).
    pub fn library_path(&self) -> Result<String> {
        Ok(self.rm()?.lib.path().to_owned())
    }

    /// The in-scope VISA functions the loaded library does not export.
    /// Operations needing them return [`Error::Unsupported`].
    ///
    /// # Errors
    ///
    /// As [`load`](Self::load).
    pub fn missing_functions(&self) -> Result<Vec<&'static str>> {
        Ok(self.rm()?.lib.missing())
    }

    pub(crate) fn rm(&self) -> Result<Arc<Rm>> {
        self.rm
            .get_or_init(|| {
                let lib = crate::loader::load(self.path.as_deref())?;
                match lib.open_default_rm() {
                    Ok((vi, _)) => Ok(Arc::new(Rm { lib, vi })),
                    Err(fail) => Err(format!(
                        "loaded {} but viOpenDefaultRM failed with {}",
                        lib.path(),
                        fail.status
                            .and_then(status_name)
                            .unwrap_or("an unknown status")
                    )),
                }
            })
            .clone()
            .map_err(|reason| Error::BackendUnavailable {
                backend: BACKEND,
                reason,
            })
    }

    /// Opens a session, returning the concrete type with its vendor-only
    /// operations (DESIGN.md V12).
    ///
    /// # Errors
    ///
    /// As [`Backend::open`].
    pub fn open_vendor(
        &self,
        resource: &ResourceName,
        options: &OpenOptions,
    ) -> Result<VendorSession> {
        if !self.supports(resource) {
            return Err(Error::Unsupported {
                backend: BACKEND,
                op: "open",
            });
        }
        let rm = self.rm()?;
        let name = c_string(&resource.to_string())
            .ok_or_else(|| Error::InvalidResource(resource.to_string()))?;
        let ctx = Context {
            resource: Some(resource),
            ..Context::default()
        };
        let (vi, _) = rm
            .lib
            .open(rm.vi, &name, VI_NO_LOCK, VI_TMO_IMMEDIATE)
            .map_err(|f| convert::error(f, &ctx, |s| rm.lib.status_desc(rm.vi, s)))?;
        tracing::debug!(%resource, "vendor: opened session");
        let mut session = VendorSession::new(rm, vi, resource.clone());
        if let Some(lock) = &options.lock {
            session.lock(lock.clone(), options.open_timeout)?;
        }
        Ok(session)
    }
}

impl Backend for VendorBackend {
    fn name(&self) -> &'static str {
        BACKEND
    }

    fn supports(&self, resource: &ResourceName) -> bool {
        matches!(
            resource,
            ResourceName::TcpipInstr { .. }
                | ResourceName::TcpipSocket { .. }
                | ResourceName::UsbInstr { .. }
                | ResourceName::GpibInstr { .. }
                | ResourceName::AsrlInstr { .. }
        )
    }

    fn open(&self, resource: &ResourceName, options: &OpenOptions) -> Result<Box<dyn Session>> {
        Ok(Box::new(self.open_vendor(resource, options)?))
    }

    fn list(&self, pattern: &str) -> Result<Vec<ResourceName>> {
        let rm = self.rm()?;
        let lib = &rm.lib;
        let expr = c_string(pattern)
            .ok_or_else(|| Error::InvalidArgument(format!("{pattern:?} contains a NUL byte")))?;
        let ((list, count, first), _) = match lib.find_rsrc(rm.vi, &expr) {
            Ok(found) => found,
            Err(f) if f.status == Some(VI_ERROR_RSRC_NFOUND) => return Ok(Vec::new()),
            Err(f) if f.status == Some(VI_ERROR_INV_EXPR) => {
                return Err(Error::InvalidArgument(format!(
                    "invalid search expression {pattern:?}"
                )));
            }
            Err(f) => {
                return Err(convert::error(f, &Context::default(), |s| {
                    lib.status_desc(rm.vi, s)
                }));
            }
        };
        let mut raw = vec![first];
        for _ in 1..count {
            match lib.find_next(list) {
                Ok((name, _)) => raw.push(name),
                Err(_) => break,
            }
        }
        lib.close(list);
        Ok(raw
            .into_iter()
            .filter_map(|name| match name.parse() {
                Ok(rn) => Some(rn),
                Err(_) => {
                    tracing::debug!(%name, "vendor: skipping resource rvisa does not support");
                    None
                }
            })
            .collect())
    }

    fn resolve_alias(&self, alias: &str) -> Result<Option<ResourceName>> {
        let rm = self.rm()?;
        let Some(name) = c_string(alias) else {
            return Ok(None);
        };
        match rm.lib.parse_rsrc_ex(rm.vi, &name) {
            Ok((expanded, _)) => Ok(expanded.parse().ok()),
            Err(f) if f.status.is_none() => Ok(None),
            Err(f)
                if f.status == Some(VI_ERROR_RSRC_NFOUND)
                    || f.status == Some(VI_ERROR_INV_RSRC_NAME) =>
            {
                Ok(None)
            }
            Err(f) => Err(convert::error(f, &Context::default(), |s| {
                rm.lib.status_desc(rm.vi, s)
            })),
        }
    }
}
