//! Events, delivered through a queue (see ROADMAP D7).

/// A kind of event a session can queue.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// The device requested service (SRQ).
    ServiceRequest,
    /// A trigger was received.
    Trigger,
    /// A device clear was received.
    Clear,
    /// An error condition occurred in the backend.
    Exception,
    /// A USB interrupt arrived.
    UsbInterrupt,
}

/// An event taken from a session's queue.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// What happened.
    pub kind: EventKind,
}

impl Event {
    /// An event of the given kind, for backends to return.
    #[must_use]
    pub fn new(kind: EventKind) -> Self {
        Self { kind }
    }
}
