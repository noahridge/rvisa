//! The `Session` trait every backend implements, and convenience methods
//! built on it.

use std::time::Duration;

use crate::attr::{Attr, AttrValue};
use crate::error::{Error, Result};
use crate::event::{Event, EventKind};
use crate::ieee488::{self, BlockHeader};
use crate::resource::ResourceName;

/// Why a read stopped.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TermReason {
    /// The device signalled the end of the message (END, EOI, EOM).
    End,
    /// The read termination character was received.
    TermChar,
    /// The requested number of bytes was read; the message may continue.
    MaxCount,
}

impl TermReason {
    /// Whether the read ended the message (anything but [`MaxCount`](Self::MaxCount)).
    #[must_use]
    pub fn is_message_end(self) -> bool {
        self != Self::MaxCount
    }
}

/// The result of [`Session::read`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadResult {
    /// The bytes read, including any termination character.
    pub data: Vec<u8>,
    /// Why the read stopped.
    pub reason: TermReason,
}

/// A lock on a resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LockKind {
    /// Only this session may access the resource.
    Exclusive,
    /// Sessions that lock with the same key share access; others are locked
    /// out.
    Shared {
        /// The access key. Other sessions pass the same key to join the lock.
        key: String,
    },
}

/// Options for opening a session.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenOptions {
    /// Lock to acquire while opening.
    pub lock: Option<LockKind>,
    /// How long to wait for [`lock`](Self::lock), if one is requested.
    pub open_timeout: Duration,
}

impl OpenOptions {
    /// Default options: no lock.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquires `lock` while opening.
    #[must_use]
    pub fn lock(mut self, lock: LockKind) -> Self {
        self.lock = Some(lock);
        self
    }

    /// Waits up to `timeout` for the lock.
    #[must_use]
    pub fn open_timeout(mut self, timeout: Duration) -> Self {
        self.open_timeout = timeout;
        self
    }
}

/// One open connection to an instrument. Every backend implements this.
///
/// Mutating operations take `&mut self`; a session can move between
/// threads (`Send`) but is not shared (`!Sync` is allowed).
///
/// Operations a backend cannot perform return [`Error::Unsupported`].
pub trait Session: Send {
    /// The resource this session is connected to.
    fn resource(&self) -> &ResourceName;

    /// Writes `data` as given, as one message (END with the last byte where
    /// the transport has END). Returns the number of bytes written.
    ///
    /// # Errors
    ///
    /// Transport errors, [`Error::Timeout`], or [`Error::ResourceLocked`].
    fn write(&mut self, data: &[u8]) -> Result<usize>;

    /// Reads up to `max` bytes, stopping at END, at the read termination
    /// character (if enabled), or after `max` bytes.
    ///
    /// # Errors
    ///
    /// [`Error::Timeout`] if no data arrives in time, or transport errors.
    fn read(&mut self, max: usize) -> Result<ReadResult>;

    /// Sets the I/O timeout; `None` is infinite.
    ///
    /// # Errors
    ///
    /// Backend-specific.
    fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<()>;

    /// The I/O timeout; `None` is infinite.
    fn timeout(&self) -> Option<Duration>;

    /// Sets the read termination character, or disables it with `None`.
    ///
    /// # Errors
    ///
    /// Backend-specific.
    fn set_read_termination(&mut self, term: Option<u8>) -> Result<()>;

    /// The bytes [`SessionExt::write_str`] and [`SessionExt::query`] append.
    ///
    /// Write termination is handled by the facade, not by backends
    /// (ROADMAP D9), so backends keep this default of `None`.
    fn write_termination(&self) -> Option<&[u8]> {
        None
    }

    /// Sets the write termination.
    ///
    /// # Errors
    ///
    /// The default (used by backends) returns [`Error::Unsupported`]; the
    /// facade's session wrapper implements it.
    fn set_write_termination(&mut self, term: Option<&[u8]>) -> Result<()> {
        let _ = term;
        Err(Error::Unsupported {
            backend: "backend",
            op: "set_write_termination",
        })
    }

    /// Sends a device clear.
    ///
    /// # Errors
    ///
    /// Transport errors.
    fn clear(&mut self) -> Result<()>;

    /// Reads the status byte (serial poll).
    ///
    /// # Errors
    ///
    /// Transport errors, or [`Error::Unsupported`].
    fn read_stb(&mut self) -> Result<u8>;

    /// Sends a trigger.
    ///
    /// # Errors
    ///
    /// Transport errors, or [`Error::Unsupported`].
    fn assert_trigger(&mut self) -> Result<()>;

    /// Locks the resource, waiting up to `timeout` for other sessions'
    /// locks. Locks nest; each `lock` needs an [`unlock`](Self::unlock).
    ///
    /// # Errors
    ///
    /// [`Error::ResourceLocked`] if the lock is not acquired in time.
    fn lock(&mut self, kind: LockKind, timeout: Duration) -> Result<()>;

    /// Releases one level of lock.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if the session holds no lock.
    fn unlock(&mut self) -> Result<()>;

    /// Reads an attribute.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] if the backend does not have the attribute.
    fn get_attr(&self, attr: Attr) -> Result<AttrValue>;

    /// Writes an attribute.
    ///
    /// # Errors
    ///
    /// [`Error::AttrReadOnly`], [`Error::InvalidArgument`] for a value of
    /// the wrong type, or [`Error::Unsupported`].
    fn set_attr(&mut self, attr: Attr, value: AttrValue) -> Result<()>;

    /// Starts queueing events of kind `kind`.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] if the backend cannot deliver this event.
    fn enable_event(&mut self, kind: EventKind) -> Result<()>;

    /// Waits up to `timeout` for a queued event of kind `kind`.
    ///
    /// # Errors
    ///
    /// [`Error::EventNotEnabled`] or [`Error::Timeout`].
    fn wait_event(&mut self, kind: EventKind, timeout: Duration) -> Result<Event>;

    /// Stops queueing events of kind `kind` and discards those queued.
    ///
    /// # Errors
    ///
    /// Backend-specific.
    fn disable_event(&mut self, kind: EventKind) -> Result<()>;
}

/// Forwards every method, so `Box<dyn Session>` is itself a [`Session`].
impl<S: Session + ?Sized> Session for Box<S> {
    fn resource(&self) -> &ResourceName {
        (**self).resource()
    }
    fn write(&mut self, data: &[u8]) -> Result<usize> {
        (**self).write(data)
    }
    fn read(&mut self, max: usize) -> Result<ReadResult> {
        (**self).read(max)
    }
    fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<()> {
        (**self).set_timeout(timeout)
    }
    fn timeout(&self) -> Option<Duration> {
        (**self).timeout()
    }
    fn set_read_termination(&mut self, term: Option<u8>) -> Result<()> {
        (**self).set_read_termination(term)
    }
    fn write_termination(&self) -> Option<&[u8]> {
        (**self).write_termination()
    }
    fn set_write_termination(&mut self, term: Option<&[u8]>) -> Result<()> {
        (**self).set_write_termination(term)
    }
    fn clear(&mut self) -> Result<()> {
        (**self).clear()
    }
    fn read_stb(&mut self) -> Result<u8> {
        (**self).read_stb()
    }
    fn assert_trigger(&mut self) -> Result<()> {
        (**self).assert_trigger()
    }
    fn lock(&mut self, kind: LockKind, timeout: Duration) -> Result<()> {
        (**self).lock(kind, timeout)
    }
    fn unlock(&mut self) -> Result<()> {
        (**self).unlock()
    }
    fn get_attr(&self, attr: Attr) -> Result<AttrValue> {
        (**self).get_attr(attr)
    }
    fn set_attr(&mut self, attr: Attr, value: AttrValue) -> Result<()> {
        (**self).set_attr(attr, value)
    }
    fn enable_event(&mut self, kind: EventKind) -> Result<()> {
        (**self).enable_event(kind)
    }
    fn wait_event(&mut self, kind: EventKind, timeout: Duration) -> Result<Event> {
        (**self).wait_event(kind, timeout)
    }
    fn disable_event(&mut self, kind: EventKind) -> Result<()> {
        (**self).disable_event(kind)
    }
}

/// The size of each read issued by the [`SessionExt`] helpers.
pub const READ_CHUNK: usize = 64 * 1024;

/// Convenience methods for every [`Session`], built only on its methods.
pub trait SessionExt: Session {
    /// Writes all of `data`, repeating [`Session::write`] if the backend
    /// accepts it in parts.
    ///
    /// # Errors
    ///
    /// Errors from [`Session::write`], or [`Error::Io`] if the backend
    /// accepts no data.
    fn write_all(&mut self, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            let n = self.write(data)?;
            if n == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into());
            }
            data = data.get(n..).unwrap_or_default();
        }
        Ok(())
    }

    /// Writes `command` followed by the [write
    /// termination](Session::write_termination), as one message.
    ///
    /// # Errors
    ///
    /// As [`write_all`](Self::write_all).
    fn write_str(&mut self, command: &str) -> Result<()> {
        let mut message = command.as_bytes().to_vec();
        if let Some(term) = self.write_termination() {
            message.extend_from_slice(term);
        }
        self.write_all(&message)
    }

    /// Reads one whole message: until END or the termination character.
    ///
    /// # Errors
    ///
    /// Errors from [`Session::read`].
    fn read_message(&mut self) -> Result<Vec<u8>> {
        let mut message = Vec::new();
        loop {
            let chunk = self.read(READ_CHUNK)?;
            message.extend_from_slice(&chunk.data);
            if chunk.reason.is_message_end() {
                return Ok(message);
            }
        }
    }

    /// Sends `command` (with write termination) and returns the response as
    /// text, without trailing `\r`/`\n`. Invalid UTF-8 is replaced.
    ///
    /// # Errors
    ///
    /// Errors from writing or reading.
    fn query(&mut self, command: &str) -> Result<String> {
        self.write_str(command)?;
        let response = self.read_message()?;
        let text = String::from_utf8_lossy(&response);
        Ok(text.trim_end_matches(['\r', '\n']).to_owned())
    }

    /// Sends `command` exactly as given (no write termination) and returns
    /// the raw response message.
    ///
    /// # Errors
    ///
    /// Errors from writing or reading.
    fn query_bytes(&mut self, command: &[u8]) -> Result<Vec<u8>> {
        self.write_all(command)?;
        self.read_message()
    }

    /// Reads an IEEE 488.2 block (`#<n><len><data>` or `#0<data>`) and
    /// returns its data, consuming the rest of the message.
    ///
    /// Data may contain the termination character; definite-length blocks
    /// are read by length. Indefinite-length blocks need END, so they do not
    /// work on transports without it.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidData`] for a malformed block or one cut short, and
    /// errors from reading.
    fn read_ieee488_block(&mut self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        let mut last = TermReason::MaxCount;

        let header = loop {
            if let Some(header) = ieee488::parse_header(&buf)? {
                break header;
            }
            if last == TermReason::End {
                return Err(Error::InvalidData(
                    "IEEE 488.2 block: message ended before the header".into(),
                ));
            }
            let chunk = self.read(READ_CHUNK)?;
            buf.extend_from_slice(&chunk.data);
            last = chunk.reason;
        };

        match header {
            BlockHeader::Definite { data_start, len } => {
                let end = data_start.saturating_add(len);
                while buf.len() < end {
                    if last == TermReason::End {
                        return Err(Error::InvalidData(format!(
                            "IEEE 488.2 block: message ended after {} of {len} bytes",
                            buf.len() - data_start
                        )));
                    }
                    let chunk = self.read((end - buf.len()).min(READ_CHUNK))?;
                    buf.extend_from_slice(&chunk.data);
                    last = chunk.reason;
                }
                // The message is complete if END arrived, or the term char
                // arrived after the data (not as its last byte).
                let complete =
                    last == TermReason::End || (last == TermReason::TermChar && buf.len() > end);
                if !complete {
                    self.read_message()?;
                }
                buf.truncate(end);
                buf.drain(..data_start);
                Ok(buf)
            }
            BlockHeader::Indefinite { data_start } => {
                while last != TermReason::End {
                    let chunk = self.read(READ_CHUNK)?;
                    buf.extend_from_slice(&chunk.data);
                    last = chunk.reason;
                }
                if buf.last() == Some(&b'\n') {
                    buf.pop();
                }
                buf.drain(..data_start.min(buf.len()));
                Ok(buf)
            }
        }
    }

    /// Sends `command` (with write termination) and reads an IEEE 488.2
    /// block response.
    ///
    /// # Errors
    ///
    /// As [`write_str`](Self::write_str) and
    /// [`read_ieee488_block`](Self::read_ieee488_block).
    fn query_ieee488_block(&mut self, command: &str) -> Result<Vec<u8>> {
        self.write_str(command)?;
        self.read_ieee488_block()
    }
}

impl<S: Session + ?Sized> SessionExt for S {}

/// A backend: something that can open sessions to some kinds of resource.
pub trait Backend: Send + Sync {
    /// A short, stable name such as `"vendor"`, `"socket"` or `"mock"`.
    fn name(&self) -> &'static str;

    /// Whether this backend can open `resource`.
    fn supports(&self, resource: &ResourceName) -> bool;

    /// Opens a session.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`], [`Error::ResourceLocked`],
    /// [`Error::BackendUnavailable`] and transport errors.
    fn open(&self, resource: &ResourceName, options: &OpenOptions) -> Result<Box<dyn Session>>;

    /// Lists resources matching a VISA search expression (see
    /// [`Pattern`](crate::Pattern)). May return an empty list.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] for a malformed expression, or discovery
    /// errors.
    fn list(&self, pattern: &str) -> Result<Vec<ResourceName>>;

    /// Resolves a name that is not a resource string, such as a VISA alias
    /// configured in a vendor tool. The default knows no aliases.
    ///
    /// # Errors
    ///
    /// Backend errors, such as [`Error::BackendUnavailable`].
    fn resolve_alias(&self, alias: &str) -> Result<Option<ResourceName>> {
        let _ = alias;
        Ok(None)
    }
}
