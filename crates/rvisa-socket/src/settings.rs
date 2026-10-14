//! Options and per-session settings shared by the blocking and async
//! sessions.

use std::time::Duration;

use rvisa_core::{Attr, AttrValue, Error, InterfaceType, ResourceName, Result};

use crate::BACKEND;

/// `VI_ATTR_IO_PROT`, reachable as [`Attr::Raw`] (DESIGN.md S5).
pub const ATTR_IO_PROT: u32 = 0x3FFF_001C;
/// `VI_PROT_NORMAL`: `read_stb` and `assert_trigger` are unsupported.
pub const PROT_NORMAL: u64 = 1;
/// `VI_PROT_4882_STRS`: `read_stb` sends `*STB?`, `assert_trigger` sends
/// `*TRG`.
pub const PROT_4882_STRS: u64 = 4;

/// The default I/O timeout, as in NI-VISA.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
/// The default connect timeout.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The longest `*STB?` reply accepted.
pub(crate) const STB_REPLY_MAX: usize = 64;

/// Options a [`SocketBackend`](crate::SocketBackend) gives its sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    pub connect_timeout: Duration,
    pub nodelay: bool,
    pub keepalive: bool,
    pub strings: bool,
    pub clear_command: Option<Vec<u8>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            nodelay: true,
            keepalive: true,
            strings: false,
            clear_command: None,
        }
    }
}

/// A session's settings that do not live in the socket itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Settings {
    pub timeout: Option<Duration>,
    pub term_char: u8,
    pub term_enabled: bool,
    pub send_end: bool,
    pub strings: bool,
    pub clear_command: Option<Vec<u8>>,
}

impl Settings {
    pub fn new(options: &Options) -> Self {
        Self {
            timeout: Some(DEFAULT_TIMEOUT),
            term_char: b'\n',
            term_enabled: false,
            send_end: true,
            strings: options.strings,
            clear_command: options.clear_command.clone(),
        }
    }

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
            Attr::Raw(ATTR_IO_PROT) => AttrValue::U64(if self.strings {
                PROT_4882_STRS
            } else {
                PROT_NORMAL
            }),
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
            // Stored but without effect: a socket has no END (S3).
            (Attr::SendEndEnabled, AttrValue::Bool(b)) => self.send_end = *b,
            (Attr::TcpipNodelay | Attr::TcpipKeepalive, AttrValue::Bool(_)) => return Ok(false),
            (Attr::Raw(ATTR_IO_PROT), v) => {
                self.strings = match v.as_u32().map(u64::from) {
                    Some(PROT_NORMAL) => false,
                    Some(PROT_4882_STRS) => true,
                    _ => {
                        return Err(Error::InvalidArgument(format!(
                            "IO protocol {v:?}: the socket backend supports \
                             {PROT_NORMAL} (normal) and {PROT_4882_STRS} (488.2 strings)"
                        )));
                    }
                };
            }
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

/// Parses a `*STB?` reply: a decimal number 0–255, optionally signed `+`,
/// with surrounding whitespace (DESIGN.md S5).
pub(crate) fn parse_stb(reply: &[u8]) -> Result<u8> {
    let text = String::from_utf8_lossy(reply);
    let trimmed = text.trim();
    trimmed
        .strip_prefix('+')
        .unwrap_or(trimmed)
        .parse::<u8>()
        .map_err(|_| Error::InvalidData(format!("*STB? reply {trimmed:?} is not a status byte")))
}

/// Converts a timeout for `set_read_timeout`/`set_write_timeout`, which
/// reject zero.
pub(crate) fn socket_timeout(t: Duration) -> Duration {
    t.max(Duration::from_millis(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stb_replies() {
        assert_eq!(parse_stb(b"64\n").unwrap(), 64);
        assert_eq!(parse_stb(b" +0\r\n").unwrap(), 0);
        assert_eq!(parse_stb(b"255").unwrap(), 255);
        for bad in [&b"256\n"[..], b"-1", b"", b"1e2", b"\xff"] {
            assert!(
                matches!(parse_stb(bad), Err(Error::InvalidData(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn attributes() {
        let rn: ResourceName = "TCPIP0::h::5025::SOCKET".parse().unwrap();
        let mut s = Settings::new(&Options::default());
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

        let prot = Attr::Raw(ATTR_IO_PROT);
        s.set(prot, &AttrValue::U16(4)).unwrap();
        assert!(s.strings);
        assert_eq!(
            s.get(prot, &rn).unwrap(),
            Some(AttrValue::U64(PROT_4882_STRS))
        );
        s.set(prot, &AttrValue::U64(1)).unwrap();
        assert!(!s.strings);
        assert!(matches!(
            s.set(prot, &AttrValue::U64(2)),
            Err(Error::InvalidArgument(_))
        ));
        assert!(matches!(
            s.set(Attr::Raw(1), &AttrValue::U64(2)),
            Err(Error::Unsupported { .. })
        ));
    }
}
