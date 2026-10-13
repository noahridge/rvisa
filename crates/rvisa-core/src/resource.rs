//! VISA resource names.

use std::fmt;
use std::str::FromStr;

use crate::error::{Error, Result};

/// A parsed VISA resource string.
///
/// Parsing is case-insensitive for interface prefixes and the class suffix,
/// and accepts the optional parts VISA allows (board number, `::INSTR`).
/// [`Display`](fmt::Display) writes the canonical form, and the two
/// round-trip: `name.to_string().parse() == Ok(name)`.
///
/// ```
/// use rvisa_core::ResourceName;
///
/// let rn: ResourceName = "tcpip::192.168.1.50::5025::socket".parse()?;
/// assert_eq!(rn.to_string(), "TCPIP0::192.168.1.50::5025::SOCKET");
/// # Ok::<(), rvisa_core::Error>(())
/// ```
///
/// Forms other than these are a parse error, never a panic. VISA aliases
/// (names configured in a vendor tool) are not resource names; resolving
/// them is a backend's job.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResourceName {
    /// `TCPIP[board]::host[::device][::INSTR]`: VXI-11 or HiSLIP.
    TcpipInstr {
        /// Board number.
        board: u16,
        /// Host name or address (IPv6 without brackets).
        host: String,
        /// LAN device name, such as `inst0` or `hislip0`.
        device: Option<String>,
    },
    /// `TCPIP[board]::host::port::SOCKET`: a raw TCP socket.
    TcpipSocket {
        /// Board number.
        board: u16,
        /// Host name or address (IPv6 without brackets).
        host: String,
        /// TCP port.
        port: u16,
    },
    /// `USB[board]::vid::pid::serial[::interface][::INSTR]`: USBTMC.
    UsbInstr {
        /// Board number.
        board: u16,
        /// USB vendor ID.
        vid: u16,
        /// USB product ID.
        pid: u16,
        /// Serial number.
        serial: String,
        /// USB interface number.
        interface: Option<u16>,
    },
    /// `GPIB[board]::primary[::secondary][::INSTR]`.
    GpibInstr {
        /// Board number.
        board: u16,
        /// Primary address, 0–30.
        primary: u8,
        /// Secondary address, 0–30.
        secondary: Option<u8>,
    },
    /// `ASRL[board][::INSTR]`, or the alias `COM<board>`.
    AsrlInstr {
        /// Port number.
        board: u16,
    },
    /// A serial port given by OS device path: `/dev/ttyUSB0`, written
    /// canonically as `ASRL/dev/ttyUSB0::INSTR`.
    AsrlPath {
        /// The device path, starting with `/`.
        path: String,
    },
}

impl ResourceName {
    /// The board (interface) number, or `None` for [`ResourceName::AsrlPath`].
    #[must_use]
    pub fn board(&self) -> Option<u16> {
        match self {
            Self::TcpipInstr { board, .. }
            | Self::TcpipSocket { board, .. }
            | Self::UsbInstr { board, .. }
            | Self::GpibInstr { board, .. }
            | Self::AsrlInstr { board } => Some(*board),
            Self::AsrlPath { .. } => None,
        }
    }
}

impl FromStr for ResourceName {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        parse(s)
    }
}

impl fmt::Display for ResourceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TcpipInstr {
                board,
                host,
                device,
            } => {
                write!(f, "TCPIP{board}::{}", HostDisplay(host))?;
                if let Some(device) = device {
                    write!(f, "::{device}")?;
                }
                f.write_str("::INSTR")
            }
            Self::TcpipSocket { board, host, port } => {
                write!(f, "TCPIP{board}::{}::{port}::SOCKET", HostDisplay(host))
            }
            Self::UsbInstr {
                board,
                vid,
                pid,
                serial,
                interface,
            } => {
                write!(f, "USB{board}::0x{vid:04X}::0x{pid:04X}::{serial}")?;
                if let Some(interface) = interface {
                    write!(f, "::{interface}")?;
                }
                f.write_str("::INSTR")
            }
            Self::GpibInstr {
                board,
                primary,
                secondary,
            } => {
                write!(f, "GPIB{board}::{primary}")?;
                if let Some(secondary) = secondary {
                    write!(f, "::{secondary}")?;
                }
                f.write_str("::INSTR")
            }
            Self::AsrlInstr { board } => write!(f, "ASRL{board}::INSTR"),
            Self::AsrlPath { path } => write!(f, "ASRL{path}::INSTR"),
        }
    }
}

/// Writes a host, bracketing it if it contains `:` (IPv6).
struct HostDisplay<'a>(&'a str);

impl fmt::Display for HostDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.contains(':') {
            write!(f, "[{}]", self.0)
        } else {
            f.write_str(self.0)
        }
    }
}

/// The highest GPIB primary or secondary address.
const GPIB_MAX_ADDRESS: u8 = 30;

fn invalid(input: &str, reason: &str) -> Error {
    Error::InvalidResource(format!("{input:?}: {reason}"))
}

fn parse(input: &str) -> Result<ResourceName> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(invalid(input, "empty resource name"));
    }
    // A bare OS device path is a serial port alias.
    let owned;
    let s = if trimmed.starts_with('/') {
        owned = format!("ASRL{trimmed}");
        owned.as_str()
    } else {
        trimmed
    };

    let mut parts = split(s).map_err(|reason| invalid(input, reason))?;
    let first = parts.remove(0);
    let class = take_class(&mut parts).map_err(|reason| invalid(input, reason))?;

    let prefix_len = first
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(first.len());
    let (interface, board_text) = first.split_at(prefix_len);
    let interface = interface.to_ascii_uppercase();

    let board = || {
        if board_text.is_empty() {
            Ok(0)
        } else {
            parse_decimal::<u16>(board_text).ok_or("invalid board number")
        }
    };
    let result = match interface.as_str() {
        "TCPIP" => board().and_then(|b| parse_tcpip(b, class, &parts)),
        "USB" => board().and_then(|b| parse_usb(b, class, &parts)),
        "GPIB" => board().and_then(|b| parse_gpib(b, class, &parts)),
        "ASRL" if board_text.starts_with('/') => parse_asrl_path(board_text, class, &parts),
        "ASRL" | "COM" => board().and_then(|b| parse_asrl(b, class, &parts)),
        "VXI" | "PXI" => Err("VXI and PXI resources are not supported"),
        "" => Err("missing interface type"),
        _ => Err("unknown interface type"),
    };
    result.map_err(|reason| invalid(input, reason))
}

/// Resource classes, as they appear at the end of a resource string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Instr,
    Socket,
}

/// Removes and returns the class suffix. A missing class means `INSTR`.
fn take_class(parts: &mut Vec<&str>) -> std::result::Result<Class, &'static str> {
    let Some(last) = parts.last() else {
        return Ok(Class::Instr);
    };
    let class = match last.to_ascii_uppercase().as_str() {
        "INSTR" => Class::Instr,
        "SOCKET" => Class::Socket,
        "INTFC" | "RAW" | "BACKPLANE" | "SERVANT" | "MEMACC" => {
            return Err("unsupported resource class");
        }
        _ => return Ok(Class::Instr),
    };
    parts.pop();
    Ok(class)
}

/// Splits on `::`, keeping a bracketed IPv6 address as one component.
fn split(s: &str) -> std::result::Result<Vec<&str>, &'static str> {
    let mut parts = Vec::new();
    let mut rest = s;
    loop {
        let (part, after) = if rest.starts_with('[') {
            let end = rest.find(']').ok_or("unterminated '[' in host")?;
            let (part, after) = rest.split_at(end + 1);
            if !after.is_empty() && !after.starts_with("::") {
                return Err("expected '::' after ']'");
            }
            (part, after.strip_prefix("::"))
        } else {
            match rest.find("::") {
                Some(i) => (&rest[..i], Some(&rest[i + 2..])),
                None => (rest, None),
            }
        };
        validate_component(part)?;
        parts.push(part);
        match after {
            Some(next) => rest = next,
            None => return Ok(parts),
        }
    }
}

fn validate_component(part: &str) -> std::result::Result<(), &'static str> {
    if part.is_empty() {
        return Err("empty component");
    }
    if part.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("whitespace or control character in a component");
    }
    if !part.starts_with('[') && (part.starts_with(':') || part.ends_with(':')) {
        return Err("component starts or ends with ':'");
    }
    Ok(())
}

/// Parses a decimal number made only of ASCII digits.
fn parse_decimal<T: FromStr>(text: &str) -> Option<T> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Parses `0x`-prefixed hex or decimal.
fn parse_hex_or_decimal(text: &str) -> Option<u16> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) if !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
            u16::from_str_radix(hex, 16).ok()
        }
        Some(_) => None,
        None => parse_decimal(text),
    }
}

fn parse_host(part: &str) -> std::result::Result<String, &'static str> {
    if let Some(inner) = part.strip_prefix('[') {
        let inner = inner.strip_suffix(']').ok_or("unterminated '[' in host")?;
        if inner.is_empty() || inner.contains(['[', ']']) {
            return Err("invalid bracketed host");
        }
        return Ok(inner.to_owned());
    }
    if part.contains(['[', ']']) {
        return Err("'[' or ']' in host");
    }
    Ok(part.to_owned())
}

fn reject_brackets(part: &str) -> std::result::Result<(), &'static str> {
    if part.contains(['[', ']']) {
        return Err("'[' or ']' outside a host");
    }
    Ok(())
}

fn parse_tcpip(
    board: u16,
    class: Class,
    parts: &[&str],
) -> std::result::Result<ResourceName, &'static str> {
    match (class, parts) {
        (Class::Socket, [host, port]) => {
            reject_brackets(port)?;
            let port = parse_decimal::<u16>(port)
                .filter(|&p| p != 0)
                .ok_or("invalid TCP port")?;
            Ok(ResourceName::TcpipSocket {
                board,
                host: parse_host(host)?,
                port,
            })
        }
        (Class::Socket, _) => Err("SOCKET needs a host and a port"),
        (Class::Instr, [host]) => Ok(ResourceName::TcpipInstr {
            board,
            host: parse_host(host)?,
            device: None,
        }),
        (Class::Instr, [host, device]) => {
            reject_brackets(device)?;
            Ok(ResourceName::TcpipInstr {
                board,
                host: parse_host(host)?,
                device: Some((*device).to_owned()),
            })
        }
        (Class::Instr, []) => Err("missing host"),
        (Class::Instr, _) => Err("too many components for TCPIP INSTR"),
    }
}

fn parse_usb(
    board: u16,
    class: Class,
    parts: &[&str],
) -> std::result::Result<ResourceName, &'static str> {
    if class != Class::Instr {
        return Err("USB supports only INSTR");
    }
    let (vid, pid, serial, interface) = match parts {
        [vid, pid, serial] => (vid, pid, serial, None),
        [vid, pid, serial, interface] => (vid, pid, serial, Some(interface)),
        _ => return Err("USB INSTR needs vendor ID, product ID and serial number"),
    };
    for part in parts {
        reject_brackets(part)?;
    }
    Ok(ResourceName::UsbInstr {
        board,
        vid: parse_hex_or_decimal(vid).ok_or("invalid USB vendor ID")?,
        pid: parse_hex_or_decimal(pid).ok_or("invalid USB product ID")?,
        serial: (*serial).to_owned(),
        interface: interface
            .map(|i| parse_decimal(i).ok_or("invalid USB interface number"))
            .transpose()?,
    })
}

fn parse_gpib(
    board: u16,
    class: Class,
    parts: &[&str],
) -> std::result::Result<ResourceName, &'static str> {
    if class != Class::Instr {
        return Err("GPIB supports only INSTR");
    }
    let address = |text: &str| {
        parse_decimal::<u8>(text)
            .filter(|&a| a <= GPIB_MAX_ADDRESS)
            .ok_or("GPIB address must be 0-30")
    };
    match parts {
        [primary] => Ok(ResourceName::GpibInstr {
            board,
            primary: address(primary)?,
            secondary: None,
        }),
        [primary, secondary] => Ok(ResourceName::GpibInstr {
            board,
            primary: address(primary)?,
            secondary: Some(address(secondary)?),
        }),
        [] => Err("missing GPIB primary address"),
        _ => Err("too many components for GPIB INSTR"),
    }
}

fn parse_asrl(
    board: u16,
    class: Class,
    parts: &[&str],
) -> std::result::Result<ResourceName, &'static str> {
    if class != Class::Instr {
        return Err("ASRL supports only INSTR");
    }
    if !parts.is_empty() {
        return Err("too many components for ASRL INSTR");
    }
    Ok(ResourceName::AsrlInstr { board })
}

fn parse_asrl_path(
    path: &str,
    class: Class,
    parts: &[&str],
) -> std::result::Result<ResourceName, &'static str> {
    if class != Class::Instr {
        return Err("ASRL supports only INSTR");
    }
    if !parts.is_empty() {
        return Err("too many components for ASRL INSTR");
    }
    reject_brackets(path)?;
    Ok(ResourceName::AsrlPath {
        path: path.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> ResourceName {
        s.parse()
            .unwrap_or_else(|e| panic!("{s:?} should parse: {e}"))
    }

    fn bad(s: &str) {
        let result = s.parse::<ResourceName>();
        assert!(
            matches!(result, Err(Error::InvalidResource(_))),
            "{s:?} should not parse, got {result:?}"
        );
    }

    /// Checks parsing and the canonical form, and that the canonical form
    /// parses back to the same value.
    fn canon(input: &str, expected: &ResourceName, canonical: &str) {
        let rn = p(input);
        assert_eq!(&rn, expected, "parsing {input:?}");
        assert_eq!(rn.to_string(), canonical, "formatting {input:?}");
        assert_eq!(p(canonical), rn, "re-parsing {canonical:?}");
    }

    fn tcpip(board: u16, host: &str, device: Option<&str>) -> ResourceName {
        ResourceName::TcpipInstr {
            board,
            host: host.into(),
            device: device.map(Into::into),
        }
    }

    #[test]
    fn tcpip_instr_forms() {
        let rn = tcpip(0, "192.168.1.50", None);
        for s in [
            "TCPIP0::192.168.1.50::INSTR",
            "TCPIP::192.168.1.50::INSTR",
            "TCPIP0::192.168.1.50",
            "tcpip::192.168.1.50::instr",
            "  TCPIP0::192.168.1.50::INSTR\n",
        ] {
            canon(s, &rn, "TCPIP0::192.168.1.50::INSTR");
        }
        canon(
            "TCPIP1::scope.local::inst0::INSTR",
            &tcpip(1, "scope.local", Some("inst0")),
            "TCPIP1::scope.local::inst0::INSTR",
        );
        canon(
            "TCPIP::Scope::hislip0",
            &tcpip(0, "Scope", Some("hislip0")),
            "TCPIP0::Scope::hislip0::INSTR",
        );
        canon(
            "TCPIP0::host::5025",
            &tcpip(0, "host", Some("5025")),
            "TCPIP0::host::5025::INSTR",
        );
    }

    #[test]
    fn tcpip_ipv6() {
        canon(
            "TCPIP0::[fe80::1]::INSTR",
            &tcpip(0, "fe80::1", None),
            "TCPIP0::[fe80::1]::INSTR",
        );
        canon(
            "TCPIP::[2001:db8::5]::hislip0",
            &tcpip(0, "2001:db8::5", Some("hislip0")),
            "TCPIP0::[2001:db8::5]::hislip0::INSTR",
        );
        canon(
            "TCPIP0::[::1]::5025::SOCKET",
            &ResourceName::TcpipSocket {
                board: 0,
                host: "::1".into(),
                port: 5025,
            },
            "TCPIP0::[::1]::5025::SOCKET",
        );
        canon(
            "TCPIP0::[host]",
            &tcpip(0, "host", None),
            "TCPIP0::host::INSTR",
        );
    }

    #[test]
    fn tcpip_socket() {
        let rn = ResourceName::TcpipSocket {
            board: 0,
            host: "10.0.0.2".into(),
            port: 5025,
        };
        for s in [
            "TCPIP0::10.0.0.2::5025::SOCKET",
            "tcpip::10.0.0.2::5025::socket",
        ] {
            canon(s, &rn, "TCPIP0::10.0.0.2::5025::SOCKET");
        }
    }

    #[test]
    fn usb() {
        let rn = ResourceName::UsbInstr {
            board: 0,
            vid: 0x0957,
            pid: 0x1796,
            serial: "MY123".into(),
            interface: None,
        };
        for s in [
            "USB0::0x0957::0x1796::MY123::INSTR",
            "usb::0X0957::0x1796::MY123",
            "USB::2391::6038::MY123::INSTR",
        ] {
            canon(s, &rn, "USB0::0x0957::0x1796::MY123::INSTR");
        }
        canon(
            "USB1::0x1AB1::0x04CE::DS1ZA1::3::INSTR",
            &ResourceName::UsbInstr {
                board: 1,
                vid: 0x1AB1,
                pid: 0x04CE,
                serial: "DS1ZA1".into(),
                interface: Some(3),
            },
            "USB1::0x1AB1::0x04CE::DS1ZA1::3::INSTR",
        );
        // A serial number that looks like a class name.
        canon(
            "USB0::1::2::INSTR::INSTR",
            &ResourceName::UsbInstr {
                board: 0,
                vid: 1,
                pid: 2,
                serial: "INSTR".into(),
                interface: None,
            },
            "USB0::0x0001::0x0002::INSTR::INSTR",
        );
    }

    #[test]
    fn gpib() {
        canon(
            "GPIB0::22::INSTR",
            &ResourceName::GpibInstr {
                board: 0,
                primary: 22,
                secondary: None,
            },
            "GPIB0::22::INSTR",
        );
        canon(
            "gpib1::5::30",
            &ResourceName::GpibInstr {
                board: 1,
                primary: 5,
                secondary: Some(30),
            },
            "GPIB1::5::30::INSTR",
        );
    }

    #[test]
    fn serial() {
        let asrl3 = ResourceName::AsrlInstr { board: 3 };
        for s in ["ASRL3::INSTR", "asrl3", "COM3", "com3::INSTR"] {
            canon(s, &asrl3, "ASRL3::INSTR");
        }
        canon(
            "ASRL",
            &ResourceName::AsrlInstr { board: 0 },
            "ASRL0::INSTR",
        );
        let path = ResourceName::AsrlPath {
            path: "/dev/ttyUSB0".into(),
        };
        for s in [
            "/dev/ttyUSB0",
            "ASRL/dev/ttyUSB0::INSTR",
            "asrl/dev/ttyUSB0",
            "/dev/ttyUSB0::INSTR",
        ] {
            canon(s, &path, "ASRL/dev/ttyUSB0::INSTR");
        }
    }

    #[test]
    fn board_accessor() {
        assert_eq!(p("GPIB2::1").board(), Some(2));
        assert_eq!(p("TCPIP3::h::1::SOCKET").board(), Some(3));
        assert_eq!(p("TCPIP4::h").board(), Some(4));
        assert_eq!(p("USB5::1::2::s").board(), Some(5));
        assert_eq!(p("COM6").board(), Some(6));
        assert_eq!(p("/dev/ttyS0").board(), None);
    }

    #[test]
    fn invalid_inputs() {
        for s in [
            "",
            "   ",
            "::INSTR",
            "0::host",
            "FOO0::host::INSTR",
            "VXI0::1::INSTR",
            "PXI0::1::INSTR",
            "GPIB-VXI0::1::INSTR",
            // Classes this library does not support.
            "GPIB0::INTFC",
            "USB0::0x1::0x2::s::RAW",
            "TCPIP0::host::5025::BACKPLANE",
            // Board numbers.
            "TCPIPx::host",
            "TCPIP65536::host",
            "TCPIP-1::host",
            "GPIB0x::1",
            // Empty and malformed components.
            "TCPIP0::",
            "TCPIP0::::INSTR",
            "TCPIP0::host::",
            "TCPIP0::INSTR",
            "TCPIP0::a::b::c::INSTR",
            "TCPIP0::my host::INSTR",
            "TCPIP0::host\t::INSTR",
            "TCPIP0::a:::INSTR",
            "TCPIP0:::a::INSTR",
            // Brackets.
            "TCPIP0::[fe80::1::INSTR",
            "TCPIP0::[]::INSTR",
            "TCPIP0::[a[b]]::INSTR",
            "TCPIP0::[fe80::1]x::INSTR",
            "TCPIP0::a]b::INSTR",
            "TCPIP0::host::[x]::INSTR",
            "TCPIP0::host::[x]::SOCKET",
            "USB0::[1]::2::s",
            "/dev/tty[0]",
            // Sockets.
            "TCPIP0::host::SOCKET",
            "TCPIP0::host::0::SOCKET",
            "TCPIP0::host::65536::SOCKET",
            "TCPIP0::host::http::SOCKET",
            "TCPIP0::a::1::b::SOCKET",
            // USB.
            "USB0::0x1::0x2::INSTR",
            "USB0::0x::0x2::s",
            "USB0::0xG::0x2::s",
            "USB0::0x1::0x10000::s",
            "USB0::1::2::s::x",
            "USB0::1::2::s::1::2",
            "USB0::1::2::s::SOCKET",
            // GPIB.
            "GPIB0::INSTR",
            "GPIB0::31",
            "GPIB0::1::31",
            "GPIB0::-1",
            "GPIB0::1::2::3",
            "GPIB0::1::SOCKET",
            // Serial.
            "ASRL1::2::INSTR",
            "ASRL1::SOCKET",
            "COMx",
            "ASRL/dev/a::b",
            "ASRL/dev/a::SOCKET",
        ] {
            bad(s);
        }
    }

    #[test]
    fn error_message_names_input() {
        let err = "GPIB0::99".parse::<ResourceName>().unwrap_err();
        let message = err.to_string();
        assert!(message.contains("GPIB0::99"), "{message}");
        assert!(message.contains("0-30"), "{message}");
    }
}
