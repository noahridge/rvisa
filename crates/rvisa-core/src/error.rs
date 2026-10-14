//! The error type shared by every backend.

use crate::attr::Attr;
use crate::event::EventKind;
use crate::resource::ResourceName;

/// Errors from resource parsing, backends and sessions.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The operation did not complete within the session's timeout.
    ///
    /// A read that times out returns the bytes that arrived before the
    /// timeout in `partial`. They are consumed: the next read starts after
    /// them, as with NI-VISA (DESIGN.md C12). Other operations leave it
    /// empty. Build this error with [`Error::timeout`] or
    /// [`Error::read_timeout`].
    #[non_exhaustive]
    #[error("operation timed out{}", partial_note(.partial))]
    Timeout {
        /// Bytes received before a read timed out; empty otherwise.
        partial: Vec<u8>,
    },
    /// The backend could not find the resource.
    #[error("resource not found: {0}")]
    NotFound(ResourceName),
    /// An operating-system I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The instrument or transport violated its protocol.
    #[error("{backend}: protocol error: {detail}")]
    Protocol {
        /// The backend that detected the error.
        backend: &'static str,
        /// What went wrong.
        detail: String,
    },
    /// The vendor VISA library returned an error status.
    #[error("VISA error 0x{status:08X} ({}): {desc}", name.unwrap_or("unknown status"))]
    Vendor {
        /// The raw `ViStatus`.
        status: i32,
        /// The symbolic name, such as `VI_ERROR_TMO`, if known.
        name: Option<&'static str>,
        /// The description from `viStatusDesc`.
        desc: String,
    },
    /// The backend does not implement this operation.
    #[error("{backend} does not support {op}")]
    Unsupported {
        /// The backend.
        backend: &'static str,
        /// The operation, such as `"read_stb"`.
        op: &'static str,
    },
    /// A resource string could not be parsed.
    #[error("invalid resource name {0}")]
    InvalidResource(String),
    /// A backend cannot be used, for example because the vendor VISA library
    /// is not installed.
    #[error("backend {backend} is unavailable: {reason}")]
    BackendUnavailable {
        /// The backend.
        backend: &'static str,
        /// Why, with a hint on how to fix it where possible.
        reason: String,
    },
    /// Another session holds a lock on the resource.
    #[error("the resource is locked by another session")]
    ResourceLocked,
    /// The attribute cannot be written.
    #[error("attribute {0:?} is read-only")]
    AttrReadOnly(Attr),
    /// [`Session::wait_event`](crate::Session::wait_event) was called for an
    /// event that is not enabled.
    #[error("event {0:?} is not enabled")]
    EventNotEnabled(EventKind),
    /// An argument was out of range or of the wrong type.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Data from the instrument was malformed (for example a bad IEEE 488.2
    /// block header).
    #[error("invalid data from instrument: {0}")]
    InvalidData(String),
    /// No registered backend supports the resource.
    #[error("no backend supports {0}")]
    NoBackend(ResourceName),
}

impl Error {
    /// A timeout with no data.
    #[must_use]
    pub fn timeout() -> Self {
        Self::Timeout {
            partial: Vec::new(),
        }
    }

    /// A read timeout, with the bytes that arrived before it.
    #[must_use]
    pub fn read_timeout(partial: Vec<u8>) -> Self {
        Self::Timeout { partial }
    }

    /// Whether this is a [`Timeout`](Self::Timeout).
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. })
    }

    /// For a timeout, puts `prefix` before its partial data, so a helper
    /// that read part of a message before the timeout loses nothing.
    /// Other errors are returned unchanged.
    #[must_use]
    pub fn prepend_partial(self, mut prefix: Vec<u8>) -> Self {
        match self {
            Self::Timeout { partial } => {
                prefix.extend_from_slice(&partial);
                Self::Timeout { partial: prefix }
            }
            other => other,
        }
    }
}

fn partial_note(partial: &[u8]) -> String {
    if partial.is_empty() {
        String::new()
    } else {
        format!(" after receiving {} bytes", partial.len())
    }
}

/// `Result` with [`Error`] as the error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages() {
        let vendor = Error::Vendor {
            status: -1_073_807_339,
            name: Some("VI_ERROR_TMO"),
            desc: "Timeout expired before operation completed.".into(),
        };
        assert_eq!(
            vendor.to_string(),
            "VISA error 0xBFFF0015 (VI_ERROR_TMO): Timeout expired before operation completed."
        );
        let unknown = Error::Vendor {
            status: 1,
            name: None,
            desc: String::new(),
        };
        assert!(unknown.to_string().contains("unknown status"));
        let unsupported = Error::Unsupported {
            backend: "socket",
            op: "read_stb",
        };
        assert_eq!(unsupported.to_string(), "socket does not support read_stb");
        let io: Error = std::io::Error::other("boom").into();
        assert!(matches!(io, Error::Io(_)));
    }

    #[test]
    fn timeouts_keep_partial_data() {
        assert_eq!(Error::timeout().to_string(), "operation timed out");
        let e = Error::read_timeout(b"RST".to_vec());
        assert!(e.is_timeout());
        assert_eq!(e.to_string(), "operation timed out after receiving 3 bytes");
        match e.prepend_partial(b"FI".to_vec()) {
            Error::Timeout { partial } => assert_eq!(partial, b"FIRST"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            Error::ResourceLocked.prepend_partial(b"x".to_vec()),
            Error::ResourceLocked
        ));
    }
}
