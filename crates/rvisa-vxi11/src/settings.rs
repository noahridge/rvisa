//! Backend options and per-session settings.

use std::net::SocketAddr;
use std::time::Duration;

use rvisa_core::{Attr, AttrValue, Error, InterfaceType, ResourceName, Result};

use crate::BACKEND;
use crate::proto::PORTMAP_PORT;

/// The default I/O timeout, as in NI-VISA.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
/// The default timeout for connecting and creating the link.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How much longer than the I/O timeout the client waits for a reply
/// before giving up on the device and aborting the call (DESIGN.md R5).
pub(crate) const RPC_SLACK: Duration = Duration::from_secs(1);

/// The most one `device_read` asks for. Larger reads take several calls.
pub(crate) const MAX_READ_REQUEST: usize = 16 * 1024 * 1024;

/// Options a [`Vxi11Backend`](crate::Vxi11Backend) gives its sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    pub connect_timeout: Duration,
    pub portmapper_port: u16,
    pub discovery: Option<Duration>,
    pub discovery_targets: Vec<SocketAddr>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            portmapper_port: PORTMAP_PORT,
            discovery: None,
            discovery_targets: Vec::new(),
        }
    }
}

/// A session's settings that live on the client side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Settings {
    pub timeout: Option<Duration>,
    pub term_char: u8,
    pub term_enabled: bool,
    pub send_end: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timeout: Some(DEFAULT_TIMEOUT),
            term_char: b'\n',
            term_enabled: false,
            send_end: true,
        }
    }
}

impl Settings {
    /// The term char, if reads stop at it.
    pub fn term(&self) -> Option<u8> {
        self.term_enabled.then_some(self.term_char)
    }

    pub fn set_read_termination(&mut self, term: Option<u8>) {
        if let Some(c) = term {
            self.term_char = c;
        }
        self.term_enabled = term.is_some();
    }

    /// Reads an attribute held here. Socket options (`TcpipNodelay`,
    /// `TcpipKeepalive`) are the session's job; they give `Ok(None)`.
    pub fn get(&self, attr: Attr, resource: &ResourceName) -> Result<Option<AttrValue>> {
        Ok(Some(match attr {
            Attr::TimeoutValue => AttrValue::Timeout(self.timeout),
            Attr::TermChar => AttrValue::U8(self.term_char),
            Attr::TermCharEnabled => AttrValue::Bool(self.term_enabled),
            Attr::SendEndEnabled => AttrValue::Bool(self.send_end),
            Attr::InterfaceType => AttrValue::InterfaceType(InterfaceType::Tcpip),
            Attr::ResourceName => AttrValue::Str(resource.to_string()),
            Attr::TcpipNodelay | Attr::TcpipKeepalive => return Ok(None),
            _ => return Err(unsupported("get_attr")),
        }))
    }

    /// Writes an attribute held here, after checking it is writable and of
    /// the right type. Returns `Ok(false)` for socket options, which the
    /// session applies.
    pub fn set(&mut self, attr: Attr, value: &AttrValue) -> Result<bool> {
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
        match (attr, value) {
            (Attr::TimeoutValue, AttrValue::Timeout(t)) => self.timeout = *t,
            (Attr::TermChar, AttrValue::U8(c)) => self.term_char = *c,
            (Attr::TermCharEnabled, AttrValue::Bool(b)) => self.term_enabled = *b,
            (Attr::SendEndEnabled, AttrValue::Bool(b)) => self.send_end = *b,
            (Attr::TcpipNodelay | Attr::TcpipKeepalive, AttrValue::Bool(_)) => return Ok(false),
            _ => return Err(unsupported("set_attr")),
        }
        Ok(true)
    }
}

pub(crate) fn unsupported(op: &'static str) -> Error {
    Error::Unsupported {
        backend: BACKEND,
        op,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes() {
        let rn: ResourceName = "TCPIP0::h::inst0::INSTR".parse().unwrap();
        let mut s = Settings::default();
        assert_eq!(s.term(), None);
        assert_eq!(
            s.get(Attr::ResourceName, &rn).unwrap(),
            Some(AttrValue::Str(rn.to_string()))
        );
        assert_eq!(s.get(Attr::TcpipNodelay, &rn).unwrap(), None);
        assert!(matches!(
            s.get(Attr::AsrlBaud, &rn),
            Err(Error::Unsupported { .. })
        ));
        assert!(
            s.set(Attr::TermCharEnabled, &AttrValue::Bool(true))
                .unwrap()
        );
        assert_eq!(s.term(), Some(b'\n'));
        s.set_read_termination(Some(b'\r'));
        assert_eq!(s.term(), Some(b'\r'));
        s.set_read_termination(None);
        assert_eq!((s.term(), s.term_char), (None, b'\r'));
        assert!(
            !s.set(Attr::TcpipKeepalive, &AttrValue::Bool(false))
                .unwrap()
        );
        assert!(matches!(
            s.set(Attr::ResourceName, &AttrValue::Str("x".into())),
            Err(Error::AttrReadOnly(Attr::ResourceName))
        ));
        assert!(matches!(
            s.set(Attr::TermChar, &AttrValue::Bool(true)),
            Err(Error::InvalidArgument(_))
        ));
        assert!(matches!(
            s.set(Attr::Raw(1), &AttrValue::U64(2)),
            Err(Error::Unsupported { .. })
        ));
    }
}
