//! The error type shared by every backend.

use crate::attr::Attr;
use crate::event::EventKind;
use crate::resource::ResourceName;

/// Errors from resource parsing, backends and sessions.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The operation did not complete within the session's timeout.
    #[error("operation timed out")]
    Timeout,
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
}
