//! `VendorSession`: a VISA session behind the `Session` trait.

use std::sync::Arc;
use std::time::Duration;

use rvisa_core::{
    Attr, AttrValue, Error, Event, EventKind, LockKind, ReadResult, ResourceName, Result, Session,
};

use crate::BACKEND;
use crate::backend::Rm;
use crate::convert::{self, CType, Context};
use crate::ffi::{Fail, c_string};
use crate::visa::*;

/// A session opened by [`VendorBackend`](crate::VendorBackend).
///
/// It keeps the resource manager (and the library) alive, so it stays valid
/// after the backend and the `ResourceManager` are dropped (ROADMAP D5).
#[derive(Debug)]
pub struct VendorSession {
    rm: Arc<Rm>,
    vi: ViSession,
    resource: ResourceName,
    timeout: Option<Duration>,
}

/// A GPIB Remote Enable operation for [`VendorSession::gpib_control_ren`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RenMode {
    /// Deassert REN.
    Deassert,
    /// Assert REN.
    Assert,
    /// Send Go To Local and deassert REN.
    DeassertGoToLocal,
    /// Assert REN and address the device.
    AssertAddress,
    /// Send Local Lockout.
    AssertLocalLockout,
    /// Address the device and send Local Lockout.
    AssertAddressLocalLockout,
    /// Address the device and send Go To Local.
    AddressGoToLocal,
}

impl VendorSession {
    pub(crate) fn new(rm: Arc<Rm>, vi: ViSession, resource: ResourceName) -> Self {
        let timeout = rm
            .lib
            .get_attr_number(vi, VI_ATTR_TMO_VALUE)
            .ok()
            .and_then(|(ms, _)| u32::try_from(ms & 0xFFFF_FFFF).ok())
            .map_or(Some(Duration::from_secs(2)), convert::timeout_from_ms);
        Self {
            rm,
            vi,
            resource,
            timeout,
        }
    }

    fn err(&self, fail: Fail, ctx: Context<'_>) -> Error {
        let ctx = Context {
            resource: Some(&self.resource),
            ..ctx
        };
        convert::error(fail, &ctx, |s| self.rm.lib.status_desc(self.vi, s))
    }

    fn plain(&self, fail: Fail) -> Error {
        self.err(fail, Context::default())
    }

    fn warn(op: &str, status: ViStatus) {
        if status != VI_SUCCESS {
            tracing::debug!(
                op,
                status = status_name(status).unwrap_or("unknown"),
                "vendor: completion code"
            );
        }
    }

    /// Flushes or discards formatted and low-level I/O buffers (`viFlush`).
    /// `mask` combines the `VI_*_BUF*` constants from [`visa`](crate::visa).
    ///
    /// # Errors
    ///
    /// VISA errors, or [`Error::Unsupported`].
    pub fn flush(&mut self, mask: u16) -> Result<()> {
        self.rm
            .lib
            .flush(self.vi, mask)
            .map(drop)
            .map_err(|f| self.plain(f))
    }

    /// Controls the GPIB Remote Enable line (`viGpibControlREN`).
    ///
    /// # Errors
    ///
    /// VISA errors, or [`Error::Unsupported`] for non-GPIB sessions.
    pub fn gpib_control_ren(&mut self, mode: RenMode) -> Result<()> {
        let mode = match mode {
            RenMode::Deassert => VI_GPIB_REN_DEASSERT,
            RenMode::Assert => VI_GPIB_REN_ASSERT,
            RenMode::DeassertGoToLocal => VI_GPIB_REN_DEASSERT_GTL,
            RenMode::AssertAddress => VI_GPIB_REN_ASSERT_ADDRESS,
            RenMode::AssertLocalLockout => VI_GPIB_REN_ASSERT_LLO,
            RenMode::AssertAddressLocalLockout => VI_GPIB_REN_ASSERT_ADDRESS_LLO,
            RenMode::AddressGoToLocal => VI_GPIB_REN_ADDRESS_GTL,
        };
        self.rm
            .lib
            .gpib_control_ren(self.vi, mode)
            .map(drop)
            .map_err(|f| self.plain(f))
    }

    fn event_type(kind: EventKind) -> Result<ViEventType> {
        convert::event_type(kind).ok_or(Error::Unsupported {
            backend: BACKEND,
            op: "enable_event",
        })
    }
}

impl Drop for VendorSession {
    fn drop(&mut self) {
        self.rm.lib.close(self.vi);
    }
}

impl Session for VendorSession {
    fn resource(&self) -> &ResourceName {
        &self.resource
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        tracing::trace!(resource = %self.resource, len = data.len(), data = ?String::from_utf8_lossy(data), "vendor: write");
        let (n, status) = self
            .rm
            .lib
            .write(self.vi, data)
            .map_err(|f| self.plain(f))?;
        Self::warn("viWrite", status);
        Ok(n)
    }

    fn read(&mut self, max: usize) -> Result<ReadResult> {
        if max == 0 {
            return Err(Error::InvalidArgument("read of zero bytes".into()));
        }
        let mut buf = vec![0; max];
        let (n, status) = self
            .rm
            .lib
            .read(self.vi, &mut buf)
            .map_err(|f| self.plain(f))?;
        buf.truncate(n);
        let reason = convert::term_reason(status);
        tracing::trace!(resource = %self.resource, len = n, ?reason, data = ?String::from_utf8_lossy(&buf), "vendor: read");
        Ok(ReadResult { data: buf, reason })
    }

    #[allow(clippy::useless_conversion)] // ViAttrState is u32 on 32-bit targets
    fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<()> {
        let ms = convert::timeout_ms(timeout);
        self.rm
            .lib
            .set_attr(self.vi, VI_ATTR_TMO_VALUE, ms.into())
            .map_err(|f| self.plain(f))?;
        self.timeout = convert::timeout_from_ms(ms);
        Ok(())
    }

    fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    fn set_read_termination(&mut self, term: Option<u8>) -> Result<()> {
        if let Some(c) = term {
            self.set_attr(Attr::TermChar, AttrValue::U8(c))?;
        }
        self.set_attr(Attr::TermCharEnabled, AttrValue::Bool(term.is_some()))
    }

    fn clear(&mut self) -> Result<()> {
        self.rm
            .lib
            .clear(self.vi)
            .map(drop)
            .map_err(|f| self.plain(f))
    }

    fn read_stb(&mut self) -> Result<u8> {
        let (stb, _) = self.rm.lib.read_stb(self.vi).map_err(|f| self.plain(f))?;
        // The status byte is the low 8 bits of the ViUInt16 (DESIGN.md V6).
        Ok(stb.to_le_bytes()[0])
    }

    fn assert_trigger(&mut self) -> Result<()> {
        self.rm
            .lib
            .assert_trigger(self.vi, VI_TRIG_PROT_DEFAULT)
            .map(drop)
            .map_err(|f| self.plain(f))
    }

    fn lock(&mut self, kind: LockKind, timeout: Duration) -> Result<()> {
        let ms = convert::timeout_ms(Some(timeout));
        let result = match &kind {
            LockKind::Exclusive => self.rm.lib.lock(self.vi, VI_EXCLUSIVE_LOCK, ms, None),
            LockKind::Shared { key } => {
                let key = c_string(key)
                    .ok_or_else(|| Error::InvalidArgument("lock key contains a NUL byte".into()))?;
                self.rm.lib.lock(self.vi, VI_SHARED_LOCK, ms, Some(&key))
            }
        };
        result.map(drop).map_err(|f| self.plain(f))
    }

    fn unlock(&mut self) -> Result<()> {
        self.rm
            .lib
            .unlock(self.vi)
            .map(drop)
            .map_err(|f| self.plain(f))
    }

    fn get_attr(&self, attr: Attr) -> Result<AttrValue> {
        let ctx = || Context {
            attr: Some(attr),
            ..Context::default()
        };
        let (id, ctype) = match (attr, convert::attr_spec(attr)) {
            (Attr::Raw(id), _) if is_string_attr(id) => (id, CType::Str),
            (Attr::Raw(id), _) => {
                if cfg!(target_endian = "big") {
                    return Err(Error::Unsupported {
                        backend: BACKEND,
                        op: "get_attr(Raw) on big-endian targets",
                    });
                }
                let (raw, _) = self
                    .rm
                    .lib
                    .get_attr_number(self.vi, id)
                    .map_err(|f| self.err(f, ctx()))?;
                return Ok(AttrValue::U64(raw));
            }
            (_, Some(spec)) => spec,
            (_, None) => {
                return Err(Error::Unsupported {
                    backend: BACKEND,
                    op: "get_attr",
                });
            }
        };
        if ctype == CType::Str {
            let (s, _) = self
                .rm
                .lib
                .get_attr_string(self.vi, id)
                .map_err(|f| self.err(f, ctx()))?;
            return Ok(AttrValue::Str(s));
        }
        let (raw, _) = self
            .rm
            .lib
            .get_attr_number(self.vi, id)
            .map_err(|f| self.err(f, ctx()))?;
        convert::decode(attr, convert::truncate(raw, ctype))
    }

    fn set_attr(&mut self, attr: Attr, value: AttrValue) -> Result<()> {
        if attr.is_read_only() {
            return Err(Error::AttrReadOnly(attr));
        }
        if !value.fits(attr) {
            return Err(Error::InvalidArgument(format!(
                "{attr:?} takes {:?}, not {:?}",
                attr.kind(),
                value.kind()
            )));
        }
        let id = match (attr, convert::attr_spec(attr)) {
            (Attr::Raw(id), _) => id,
            (_, Some((id, _))) => id,
            (_, None) => {
                return Err(Error::Unsupported {
                    backend: BACKEND,
                    op: "set_attr",
                });
            }
        };
        let raw = convert::encode(&value)?;
        let state = ViAttrState::try_from(raw)
            .map_err(|_| Error::InvalidArgument(format!("{raw} does not fit ViAttrState")))?;
        let ctx = Context {
            attr: Some(attr),
            ..Context::default()
        };
        self.rm
            .lib
            .set_attr(self.vi, id, state)
            .map_err(|f| self.err(f, ctx))?;
        if let (Attr::TimeoutValue, AttrValue::Timeout(t)) = (attr, &value) {
            self.timeout = convert::timeout_from_ms(convert::timeout_ms(*t));
        }
        Ok(())
    }

    fn enable_event(&mut self, kind: EventKind) -> Result<()> {
        let event = Self::event_type(kind)?;
        let ctx = Context {
            event: Some(kind),
            ..Context::default()
        };
        self.rm
            .lib
            .enable_event(self.vi, event)
            .map(drop)
            .map_err(|f| self.err(f, ctx))
    }

    fn wait_event(&mut self, kind: EventKind, timeout: Duration) -> Result<Event> {
        let event = Self::event_type(kind)?;
        let ctx = Context {
            event: Some(kind),
            ..Context::default()
        };
        self.rm
            .lib
            .wait_on_event(self.vi, event, convert::timeout_ms(Some(timeout)))
            .map_err(|f| self.err(f, ctx))?;
        Ok(Event::new(kind))
    }

    fn disable_event(&mut self, kind: EventKind) -> Result<()> {
        let event = Self::event_type(kind)?;
        let ctx = || Context {
            event: Some(kind),
            ..Context::default()
        };
        self.rm
            .lib
            .disable_event(self.vi, event)
            .map_err(|f| self.err(f, ctx()))?;
        self.rm
            .lib
            .discard_events(self.vi, event)
            .map(drop)
            .map_err(|f| self.err(f, ctx()))
    }
}
