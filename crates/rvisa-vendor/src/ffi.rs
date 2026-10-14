//! The FFI boundary: the VISA function table and one safe wrapper per
//! function. This is the only module in the crate with `unsafe` code
//! (DESIGN.md V1); every `unsafe` block states why it is sound.
//!
//! Wrappers return `Ok((value, status))` with VISA's completion code, or a
//! [`Fail`] naming the function and the error status (or `None` if the
//! library does not export the function, ROADMAP D4).

use std::ffi::{CStr, CString, c_void};
use std::path::Path;
use std::ptr;

use crate::visa::*;

/// A failed VISA call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fail {
    /// The VISA function, such as `"viRead"`.
    pub op: &'static str,
    /// The error status, or `None` if the function is not exported.
    pub status: Option<ViStatus>,
}

pub(crate) type Res<T> = Result<(T, ViStatus), Fail>;

fn check(op: &'static str, status: ViStatus) -> Result<ViStatus, Fail> {
    if status < 0 {
        Err(Fail {
            op,
            status: Some(status),
        })
    } else {
        Ok(status)
    }
}

/// VISA counts are `ViUInt32`; longer buffers are used partially.
fn clamp(len: usize) -> ViUInt32 {
    ViUInt32::try_from(len).unwrap_or(ViUInt32::MAX)
}

macro_rules! functions {
    ($($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        /// The resolved function pointers. `extern "system"` is `__stdcall`
        /// on 32-bit Windows and the C convention elsewhere, matching
        /// VISA's `_VI_FUNC`.
        #[allow(non_snake_case)]
        struct Table {
            $($name: Option<unsafe extern "system" fn($($arg),*) $(-> $ret)?>,)*
        }

        impl Table {
            fn resolve(lib: &libloading::Library) -> Self {
                Self {
                    $($name: {
                        // SAFETY: the symbol is looked up with the signature
                        // VPP-4.3 gives it, and the pointer is only used
                        // while `lib` is loaded: `Library` stores both, and
                        // the table is dropped no later than the library.
                        let symbol = unsafe {
                            lib.get::<unsafe extern "system" fn($($arg),*) $(-> $ret)?>(
                                concat!(stringify!($name), "\0").as_bytes(),
                            )
                        };
                        symbol.ok().map(|s| *s)
                    },)*
                }
            }

            fn missing(&self) -> Vec<&'static str> {
                let mut missing = Vec::new();
                $(if self.$name.is_none() {
                    missing.push(stringify!($name));
                })*
                missing
            }
        }
    };
}

functions! {
    viOpenDefaultRM: fn(*mut ViSession) -> ViStatus;
    viFindRsrc: fn(ViSession, *const ViChar, *mut ViFindList, *mut ViUInt32, *mut ViChar) -> ViStatus;
    viFindNext: fn(ViFindList, *mut ViChar) -> ViStatus;
    viParseRsrcEx: fn(ViSession, *const ViChar, *mut ViUInt16, *mut ViUInt16, *mut ViChar, *mut ViChar, *mut ViChar) -> ViStatus;
    viOpen: fn(ViSession, *const ViChar, ViAccessMode, ViUInt32, *mut ViSession) -> ViStatus;
    viClose: fn(ViObject) -> ViStatus;
    viRead: fn(ViSession, *mut u8, ViUInt32, *mut ViUInt32) -> ViStatus;
    viWrite: fn(ViSession, *const u8, ViUInt32, *mut ViUInt32) -> ViStatus;
    viGetAttribute: fn(ViObject, ViAttr, *mut c_void) -> ViStatus;
    viSetAttribute: fn(ViObject, ViAttr, ViAttrState) -> ViStatus;
    viClear: fn(ViSession) -> ViStatus;
    viReadSTB: fn(ViSession, *mut ViUInt16) -> ViStatus;
    viAssertTrigger: fn(ViSession, ViUInt16) -> ViStatus;
    viLock: fn(ViSession, ViAccessMode, ViUInt32, *const ViChar, *mut ViChar) -> ViStatus;
    viUnlock: fn(ViSession) -> ViStatus;
    viEnableEvent: fn(ViSession, ViEventType, ViUInt16, ViEventFilter) -> ViStatus;
    viDisableEvent: fn(ViSession, ViEventType, ViUInt16) -> ViStatus;
    viWaitOnEvent: fn(ViSession, ViEventType, ViUInt32, *mut ViEventType, *mut ViEvent) -> ViStatus;
    viDiscardEvents: fn(ViSession, ViEventType, ViUInt16) -> ViStatus;
    viStatusDesc: fn(ViObject, ViStatus, *mut ViChar) -> ViStatus;
    viFlush: fn(ViSession, ViUInt16) -> ViStatus;
    viGpibControlREN: fn(ViSession, ViUInt16) -> ViStatus;
}

/// Gets a function from the table, or a [`Fail`] with no status.
macro_rules! func {
    ($self:ident, $name:ident) => {
        $self.table.$name.ok_or(Fail {
            op: stringify!($name),
            status: None,
        })?
    };
}

/// A loaded VISA library.
pub(crate) struct Library {
    table: Table,
    path: String,
    // Declared last so it is dropped after `table`'s pointers.
    _lib: libloading::Library,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("path", &self.path)
            .field("missing", &self.table.missing())
            .finish()
    }
}

/// A zeroed `VI_FIND_BUFLEN` buffer with headroom for implementations that
/// exceed the specified maximum.
fn name_buf() -> [ViChar; 1024] {
    [0; 1024]
}

fn from_buf(buf: &[ViChar]) -> String {
    // The buffers are zero-initialised and longer than any VISA string, so
    // a terminator is always present; `from_bytes_until_nul` checks anyway.
    let bytes: Vec<u8> = buf.iter().map(|c| c.to_ne_bytes()[0]).collect();
    CStr::from_bytes_until_nul(&bytes)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl Library {
    /// Loads the library at `path`. Returns an error message on failure.
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        // SAFETY: loading a VISA library runs its initialisers. That is the
        // inherent cost of using vendor VISA at all (ROADMAP D1); the paths
        // come from the documented search list or the user.
        let lib = unsafe { libloading::Library::new(path) }.map_err(|e| e.to_string())?;
        let table = Table::resolve(&lib);
        if table.viOpenDefaultRM.is_none() {
            return Err("not a VISA library (no viOpenDefaultRM)".into());
        }
        Ok(Self {
            table,
            path: path.display().to_string(),
            _lib: lib,
        })
    }

    /// The path the library was loaded from.
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    /// Names of the in-scope functions the library does not export.
    pub(crate) fn missing(&self) -> Vec<&'static str> {
        self.table.missing()
    }

    pub(crate) fn open_default_rm(&self) -> Res<ViSession> {
        let f = func!(self, viOpenDefaultRM);
        let mut vi = 0;
        // SAFETY: `vi` is a valid out-pointer.
        let status = unsafe { f(&raw mut vi) };
        check("viOpenDefaultRM", status).map(|s| (vi, s))
    }

    /// Starts a search; returns the list handle, match count and first match.
    pub(crate) fn find_rsrc(&self, rm: ViSession, expr: &CStr) -> Res<(ViFindList, u32, String)> {
        let f = func!(self, viFindRsrc);
        let (mut list, mut count, mut desc) = (0, 0, name_buf());
        // SAFETY: `expr` is NUL-terminated; the out-pointers are valid and
        // `desc` exceeds VI_FIND_BUFLEN.
        let status = unsafe {
            f(
                rm,
                expr.as_ptr(),
                &raw mut list,
                &raw mut count,
                desc.as_mut_ptr(),
            )
        };
        check("viFindRsrc", status).map(|s| ((list, count, from_buf(&desc)), s))
    }

    pub(crate) fn find_next(&self, list: ViFindList) -> Res<String> {
        let f = func!(self, viFindNext);
        let mut desc = name_buf();
        // SAFETY: `desc` exceeds VI_FIND_BUFLEN.
        let status = unsafe { f(list, desc.as_mut_ptr()) };
        check("viFindNext", status).map(|s| (from_buf(&desc), s))
    }

    /// Returns the expanded (unaliased) resource name.
    pub(crate) fn parse_rsrc_ex(&self, rm: ViSession, name: &CStr) -> Res<String> {
        let f = func!(self, viParseRsrcEx);
        let (mut intf_type, mut intf_num) = (0, 0);
        let (mut class, mut expanded, mut alias) = (name_buf(), name_buf(), name_buf());
        // SAFETY: `name` is NUL-terminated; the out-pointers are valid and
        // each buffer exceeds VI_FIND_BUFLEN.
        let status = unsafe {
            f(
                rm,
                name.as_ptr(),
                &raw mut intf_type,
                &raw mut intf_num,
                class.as_mut_ptr(),
                expanded.as_mut_ptr(),
                alias.as_mut_ptr(),
            )
        };
        check("viParseRsrcEx", status).map(|s| (from_buf(&expanded), s))
    }

    pub(crate) fn open(
        &self,
        rm: ViSession,
        name: &CStr,
        mode: ViAccessMode,
        timeout: u32,
    ) -> Res<ViSession> {
        let f = func!(self, viOpen);
        let mut vi = 0;
        // SAFETY: `name` is NUL-terminated and `vi` a valid out-pointer.
        let status = unsafe { f(rm, name.as_ptr(), mode, timeout, &raw mut vi) };
        check("viOpen", status).map(|s| (vi, s))
    }

    /// Closes a session, find list or event. Errors are returned, not
    /// raised, since this is used from `Drop`.
    pub(crate) fn close(&self, vi: ViObject) -> ViStatus {
        match self.table.viClose {
            // SAFETY: closing takes only the handle; VISA validates it.
            Some(f) => unsafe { f(vi) },
            None => VI_SUCCESS,
        }
    }

    /// Returns the byte count with the result, since VISA reports bytes
    /// received before an error such as `VI_ERROR_TMO` (DESIGN.md C12).
    pub(crate) fn read(&self, vi: ViSession, buf: &mut [u8]) -> (usize, Result<ViStatus, Fail>) {
        let Some(f) = self.table.viRead else {
            let fail = Fail {
                op: "viRead",
                status: None,
            };
            return (0, Err(fail));
        };
        let mut count = 0;
        // SAFETY: `buf` is valid for `clamp(buf.len())` bytes and `count` is
        // a valid out-pointer.
        let status = unsafe { f(vi, buf.as_mut_ptr(), clamp(buf.len()), &raw mut count) };
        // Never trust the count beyond the buffer.
        ((count as usize).min(buf.len()), check("viRead", status))
    }

    pub(crate) fn write(&self, vi: ViSession, data: &[u8]) -> Res<usize> {
        let f = func!(self, viWrite);
        let mut count = 0;
        // SAFETY: `data` is valid for `clamp(data.len())` bytes and `count`
        // is a valid out-pointer.
        let status = unsafe { f(vi, data.as_ptr(), clamp(data.len()), &raw mut count) };
        let status = check("viWrite", status)?;
        Ok(((count as usize).min(data.len()), status))
    }

    /// Reads a numeric attribute of up to 8 bytes into a zeroed `u64`.
    /// The caller decides how many low-order bytes are meaningful
    /// (DESIGN.md V6).
    pub(crate) fn get_attr_number(&self, vi: ViObject, attr: ViAttr) -> Res<u64> {
        let f = func!(self, viGetAttribute);
        let mut value = 0u64;
        // SAFETY: no numeric VISA attribute is wider than 8 bytes, so VISA
        // writes within `value`.
        let status = unsafe { f(vi, attr, (&raw mut value).cast()) };
        check("viGetAttribute", status).map(|s| (value, s))
    }

    pub(crate) fn get_attr_string(&self, vi: ViObject, attr: ViAttr) -> Res<String> {
        let f = func!(self, viGetAttribute);
        let mut buf = name_buf();
        // SAFETY: string attributes are at most VI_FIND_BUFLEN bytes and
        // `buf` is larger.
        let status = unsafe { f(vi, attr, buf.as_mut_ptr().cast()) };
        check("viGetAttribute", status).map(|s| (from_buf(&buf), s))
    }

    pub(crate) fn set_attr(&self, vi: ViObject, attr: ViAttr, value: ViAttrState) -> Res<()> {
        let f = func!(self, viSetAttribute);
        // SAFETY: the value is passed by value; there are no pointers.
        let status = unsafe { f(vi, attr, value) };
        check("viSetAttribute", status).map(|s| ((), s))
    }

    pub(crate) fn clear(&self, vi: ViSession) -> Res<()> {
        let f = func!(self, viClear);
        // SAFETY: takes only the handle.
        let status = unsafe { f(vi) };
        check("viClear", status).map(|s| ((), s))
    }

    pub(crate) fn read_stb(&self, vi: ViSession) -> Res<u16> {
        let f = func!(self, viReadSTB);
        let mut stb = 0;
        // SAFETY: `stb` is a valid out-pointer.
        let status = unsafe { f(vi, &raw mut stb) };
        check("viReadSTB", status).map(|s| (stb, s))
    }

    pub(crate) fn assert_trigger(&self, vi: ViSession, protocol: ViUInt16) -> Res<()> {
        let f = func!(self, viAssertTrigger);
        // SAFETY: arguments are passed by value.
        let status = unsafe { f(vi, protocol) };
        check("viAssertTrigger", status).map(|s| ((), s))
    }

    /// Locks; `key` is the requested shared-lock key, if any.
    pub(crate) fn lock(
        &self,
        vi: ViSession,
        mode: ViAccessMode,
        timeout: u32,
        key: Option<&CStr>,
    ) -> Res<()> {
        let f = func!(self, viLock);
        let mut access_key = name_buf();
        let (requested, out) = match key {
            Some(k) => (k.as_ptr(), access_key.as_mut_ptr()),
            None => (ptr::null(), ptr::null_mut()),
        };
        // SAFETY: `requested` is NUL-terminated or null, and `out` is null
        // or a buffer exceeding VI_FIND_BUFLEN. Exclusive locks take nulls,
        // as VPP-4.3 specifies.
        let status = unsafe { f(vi, mode, timeout, requested, out) };
        check("viLock", status).map(|s| ((), s))
    }

    pub(crate) fn unlock(&self, vi: ViSession) -> Res<()> {
        let f = func!(self, viUnlock);
        // SAFETY: takes only the handle.
        let status = unsafe { f(vi) };
        check("viUnlock", status).map(|s| ((), s))
    }

    pub(crate) fn enable_event(&self, vi: ViSession, event: ViEventType) -> Res<()> {
        let f = func!(self, viEnableEvent);
        // SAFETY: arguments are passed by value.
        let status = unsafe { f(vi, event, VI_QUEUE, VI_NULL) };
        check("viEnableEvent", status).map(|s| ((), s))
    }

    pub(crate) fn disable_event(&self, vi: ViSession, event: ViEventType) -> Res<()> {
        let f = func!(self, viDisableEvent);
        // SAFETY: arguments are passed by value.
        let status = unsafe { f(vi, event, VI_QUEUE) };
        check("viDisableEvent", status).map(|s| ((), s))
    }

    pub(crate) fn discard_events(&self, vi: ViSession, event: ViEventType) -> Res<()> {
        let f = func!(self, viDiscardEvents);
        // SAFETY: arguments are passed by value.
        let status = unsafe { f(vi, event, VI_QUEUE) };
        check("viDiscardEvents", status).map(|s| ((), s))
    }

    /// Waits for a queued event and closes its context.
    pub(crate) fn wait_on_event(
        &self,
        vi: ViSession,
        event: ViEventType,
        timeout: u32,
    ) -> Res<ViEventType> {
        let f = func!(self, viWaitOnEvent);
        let (mut out_type, mut context) = (0, 0);
        // SAFETY: both out-pointers are valid.
        let status = unsafe { f(vi, event, timeout, &raw mut out_type, &raw mut context) };
        let status = check("viWaitOnEvent", status)?;
        self.close(context);
        Ok((out_type, status))
    }

    /// The description `viStatusDesc` gives for `status`.
    pub(crate) fn status_desc(&self, vi: ViObject, status: ViStatus) -> Option<String> {
        let f = self.table.viStatusDesc?;
        let mut buf = name_buf();
        // SAFETY: `buf` exceeds the VI_FIND_BUFLEN bytes VISA may write.
        let rc = unsafe { f(vi, status, buf.as_mut_ptr()) };
        (rc >= 0).then(|| from_buf(&buf))
    }

    pub(crate) fn flush(&self, vi: ViSession, mask: ViUInt16) -> Res<()> {
        let f = func!(self, viFlush);
        // SAFETY: arguments are passed by value.
        let status = unsafe { f(vi, mask) };
        check("viFlush", status).map(|s| ((), s))
    }

    pub(crate) fn gpib_control_ren(&self, vi: ViSession, mode: ViUInt16) -> Res<()> {
        let f = func!(self, viGpibControlREN);
        // SAFETY: arguments are passed by value.
        let status = unsafe { f(vi, mode) };
        check("viGpibControlREN", status).map(|s| ((), s))
    }
}

/// Converts a Rust string for VISA, rejecting interior NULs.
pub(crate) fn c_string(s: &str) -> Option<CString> {
    CString::new(s).ok()
}
