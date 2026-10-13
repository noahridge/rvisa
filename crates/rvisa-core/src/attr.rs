//! Session attributes.

use std::ops::{BitOr, BitOrAssign};
use std::time::Duration;

/// A session attribute.
///
/// The named variants are the common set every backend should understand.
/// [`Attr::Raw`] reaches any other attribute by its VISA identifier, on
/// backends that have one (the vendor backend).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Attr {
    /// I/O timeout (`VI_ATTR_TMO_VALUE`). Value: [`AttrValue::Timeout`].
    TimeoutValue,
    /// Read termination character (`VI_ATTR_TERMCHAR`). Value: [`AttrValue::U8`].
    TermChar,
    /// Whether reads stop at the termination character
    /// (`VI_ATTR_TERMCHAR_EN`). Value: [`AttrValue::Bool`].
    TermCharEnabled,
    /// Whether END is sent with the last byte of a write
    /// (`VI_ATTR_SEND_END_EN`). Value: [`AttrValue::Bool`].
    SendEndEnabled,
    /// Whether reads ignore END (`VI_ATTR_SUPPRESS_END_EN`).
    /// Value: [`AttrValue::Bool`].
    SuppressEndEnabled,
    /// Serial baud rate (`VI_ATTR_ASRL_BAUD`). Value: [`AttrValue::U32`].
    AsrlBaud,
    /// Serial data bits, 5–8 (`VI_ATTR_ASRL_DATA_BITS`). Value: [`AttrValue::U16`].
    AsrlDataBits,
    /// Serial parity (`VI_ATTR_ASRL_PARITY`). Value: [`AttrValue::Parity`].
    AsrlParity,
    /// Serial stop bits (`VI_ATTR_ASRL_STOP_BITS`). Value: [`AttrValue::StopBits`].
    AsrlStopBits,
    /// Serial flow control (`VI_ATTR_ASRL_FLOW_CNTRL`).
    /// Value: [`AttrValue::FlowControl`].
    AsrlFlowControl,
    /// TCP keepalive (`VI_ATTR_TCPIP_KEEPALIVE`). Value: [`AttrValue::Bool`].
    TcpipKeepalive,
    /// TCP no-delay (`VI_ATTR_TCPIP_NODELAY`). Value: [`AttrValue::Bool`].
    TcpipNodelay,
    /// The hardware interface (`VI_ATTR_INTF_TYPE`). Read-only.
    /// Value: [`AttrValue::InterfaceType`].
    InterfaceType,
    /// Manufacturer name (`VI_ATTR_MANF_NAME`). Read-only. Value: [`AttrValue::Str`].
    ManufacturerName,
    /// Model name (`VI_ATTR_MODEL_NAME`). Read-only. Value: [`AttrValue::Str`].
    ModelName,
    /// USB serial number (`VI_ATTR_USB_SERIAL_NUM`). Read-only.
    /// Value: [`AttrValue::Str`].
    UsbSerialNumber,
    /// The resource name (`VI_ATTR_RSRC_NAME`). Read-only. Value: [`AttrValue::Str`].
    ResourceName,
    /// Any attribute, by VISA identifier. Values are passed as
    /// [`AttrValue::U64`] or [`AttrValue::Str`].
    Raw(u32),
}

/// The type of value an attribute holds.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttrKind {
    /// [`AttrValue::Bool`].
    Bool,
    /// [`AttrValue::U8`].
    U8,
    /// [`AttrValue::U16`].
    U16,
    /// [`AttrValue::U32`].
    U32,
    /// [`AttrValue::U64`].
    U64,
    /// [`AttrValue::I32`].
    I32,
    /// [`AttrValue::Str`].
    Str,
    /// [`AttrValue::Timeout`].
    Timeout,
    /// [`AttrValue::Parity`].
    Parity,
    /// [`AttrValue::StopBits`].
    StopBits,
    /// [`AttrValue::FlowControl`].
    FlowControl,
    /// [`AttrValue::InterfaceType`].
    InterfaceType,
    /// Determined by the backend ([`Attr::Raw`]).
    Any,
}

impl Attr {
    /// The type of value this attribute holds.
    #[must_use]
    pub fn kind(self) -> AttrKind {
        match self {
            Self::TimeoutValue => AttrKind::Timeout,
            Self::TermChar => AttrKind::U8,
            Self::TermCharEnabled
            | Self::SendEndEnabled
            | Self::SuppressEndEnabled
            | Self::TcpipKeepalive
            | Self::TcpipNodelay => AttrKind::Bool,
            Self::AsrlBaud => AttrKind::U32,
            Self::AsrlDataBits => AttrKind::U16,
            Self::AsrlParity => AttrKind::Parity,
            Self::AsrlStopBits => AttrKind::StopBits,
            Self::AsrlFlowControl => AttrKind::FlowControl,
            Self::InterfaceType => AttrKind::InterfaceType,
            Self::ManufacturerName
            | Self::ModelName
            | Self::UsbSerialNumber
            | Self::ResourceName => AttrKind::Str,
            Self::Raw(_) => AttrKind::Any,
        }
    }

    /// Whether the attribute can never be written.
    #[must_use]
    pub fn is_read_only(self) -> bool {
        matches!(
            self,
            Self::InterfaceType
                | Self::ManufacturerName
                | Self::ModelName
                | Self::UsbSerialNumber
                | Self::ResourceName
        )
    }
}

/// An attribute value.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AttrValue {
    /// A flag.
    Bool(bool),
    /// An 8-bit value, such as a character.
    U8(u8),
    /// A 16-bit value.
    U16(u16),
    /// A 32-bit value.
    U32(u32),
    /// A 64-bit value; also used for raw attributes.
    U64(u64),
    /// A signed 32-bit value.
    I32(i32),
    /// Text.
    Str(String),
    /// A timeout; `None` is infinite.
    Timeout(Option<Duration>),
    /// Serial parity.
    Parity(Parity),
    /// Serial stop bits.
    StopBits(StopBits),
    /// Serial flow control.
    FlowControl(FlowControl),
    /// A hardware interface type.
    InterfaceType(InterfaceType),
}

impl AttrValue {
    /// The type of this value.
    #[must_use]
    pub fn kind(&self) -> AttrKind {
        match self {
            Self::Bool(_) => AttrKind::Bool,
            Self::U8(_) => AttrKind::U8,
            Self::U16(_) => AttrKind::U16,
            Self::U32(_) => AttrKind::U32,
            Self::U64(_) => AttrKind::U64,
            Self::I32(_) => AttrKind::I32,
            Self::Str(_) => AttrKind::Str,
            Self::Timeout(_) => AttrKind::Timeout,
            Self::Parity(_) => AttrKind::Parity,
            Self::StopBits(_) => AttrKind::StopBits,
            Self::FlowControl(_) => AttrKind::FlowControl,
            Self::InterfaceType(_) => AttrKind::InterfaceType,
        }
    }

    /// Whether this value can be stored in `attr`.
    #[must_use]
    pub fn fits(&self, attr: Attr) -> bool {
        attr.kind() == AttrKind::Any || attr.kind() == self.kind()
    }

    /// The value as a `bool`, if it is one.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The value as a `u32`, if it is an unsigned integer that fits.
    #[must_use]
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Self::U8(v) => Some((*v).into()),
            Self::U16(v) => Some((*v).into()),
            Self::U32(v) => Some(*v),
            Self::U64(v) => u32::try_from(*v).ok(),
            _ => None,
        }
    }

    /// The value as text, if it is [`AttrValue::Str`].
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The value as a timeout, if it is [`AttrValue::Timeout`].
    #[must_use]
    pub fn as_timeout(&self) -> Option<Option<Duration>> {
        match self {
            Self::Timeout(t) => Some(*t),
            _ => None,
        }
    }
}

/// Serial parity.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Parity {
    /// No parity bit.
    None,
    /// Odd parity.
    Odd,
    /// Even parity.
    Even,
    /// Parity bit always 1.
    Mark,
    /// Parity bit always 0.
    Space,
}

/// Serial stop bits.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StopBits {
    /// One stop bit.
    One,
    /// One and a half stop bits.
    OnePointFive,
    /// Two stop bits.
    Two,
}

/// Serial flow control. Values combine with `|`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FlowControl(u16);

impl FlowControl {
    /// No flow control.
    pub const NONE: Self = Self(0);
    /// XON/XOFF software flow control.
    pub const XON_XOFF: Self = Self(1);
    /// RTS/CTS hardware flow control.
    pub const RTS_CTS: Self = Self(2);
    /// DTR/DSR hardware flow control.
    pub const DTR_DSR: Self = Self(4);

    /// Whether every mechanism in `other` is enabled in `self`.
    #[must_use]
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The VISA `VI_ASRL_FLOW_*` bit mask.
    #[must_use]
    pub fn bits(self) -> u16 {
        self.0
    }

    /// Builds a value from a VISA bit mask, keeping only known bits.
    #[must_use]
    pub fn from_bits_truncate(bits: u16) -> Self {
        Self(bits & 0b111)
    }
}

impl BitOr for FlowControl {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for FlowControl {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// A hardware interface type.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterfaceType {
    /// GPIB (IEEE 488).
    Gpib,
    /// VXI.
    Vxi,
    /// GPIB-VXI.
    GpibVxi,
    /// Serial.
    Asrl,
    /// PXI.
    Pxi,
    /// TCP/IP.
    Tcpip,
    /// USB.
    Usb,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Attr; 18] = [
        Attr::TimeoutValue,
        Attr::TermChar,
        Attr::TermCharEnabled,
        Attr::SendEndEnabled,
        Attr::SuppressEndEnabled,
        Attr::AsrlBaud,
        Attr::AsrlDataBits,
        Attr::AsrlParity,
        Attr::AsrlStopBits,
        Attr::AsrlFlowControl,
        Attr::TcpipKeepalive,
        Attr::TcpipNodelay,
        Attr::InterfaceType,
        Attr::ManufacturerName,
        Attr::ModelName,
        Attr::UsbSerialNumber,
        Attr::ResourceName,
        Attr::Raw(0x3FFF_0018),
    ];

    #[test]
    fn kinds_and_read_only() {
        let read_only: Vec<_> = ALL.into_iter().filter(|a| a.is_read_only()).collect();
        assert_eq!(read_only.len(), 5);
        assert!(
            read_only
                .iter()
                .all(|a| matches!(a.kind(), AttrKind::Str | AttrKind::InterfaceType))
        );
        assert_eq!(Attr::Raw(1).kind(), AttrKind::Any);
    }

    #[test]
    fn values_fit_their_attributes() {
        let samples = [
            AttrValue::Bool(true),
            AttrValue::U8(b'\n'),
            AttrValue::U16(8),
            AttrValue::U32(9600),
            AttrValue::U64(1),
            AttrValue::I32(-1),
            AttrValue::Str("x".into()),
            AttrValue::Timeout(None),
            AttrValue::Parity(Parity::Even),
            AttrValue::StopBits(StopBits::Two),
            AttrValue::FlowControl(FlowControl::NONE),
            AttrValue::InterfaceType(InterfaceType::Usb),
        ];
        for attr in ALL {
            let fitting = samples.iter().filter(|v| v.fits(attr)).count();
            let expected = if attr.kind() == AttrKind::Any {
                samples.len()
            } else {
                1
            };
            assert_eq!(fitting, expected, "{attr:?}");
        }
    }

    #[test]
    fn accessors() {
        assert_eq!(AttrValue::Bool(true).as_bool(), Some(true));
        assert_eq!(AttrValue::U8(1).as_bool(), None);
        assert_eq!(AttrValue::U8(7).as_u32(), Some(7));
        assert_eq!(AttrValue::U16(7).as_u32(), Some(7));
        assert_eq!(AttrValue::U32(7).as_u32(), Some(7));
        assert_eq!(AttrValue::U64(u64::MAX).as_u32(), None);
        assert_eq!(AttrValue::I32(7).as_u32(), None);
        assert_eq!(AttrValue::Str("a".into()).as_str(), Some("a"));
        assert_eq!(AttrValue::U8(0).as_str(), None);
        let t = Some(Duration::from_secs(1));
        assert_eq!(AttrValue::Timeout(t).as_timeout(), Some(t));
        assert_eq!(AttrValue::U8(0).as_timeout(), None);
    }

    #[test]
    fn flow_control_flags() {
        let mut flow = FlowControl::XON_XOFF | FlowControl::RTS_CTS;
        assert!(flow.contains(FlowControl::RTS_CTS));
        assert!(!flow.contains(FlowControl::DTR_DSR));
        flow |= FlowControl::DTR_DSR;
        assert_eq!(flow.bits(), 7);
        assert_eq!(FlowControl::from_bits_truncate(0xFF), flow);
        assert_eq!(FlowControl::default(), FlowControl::NONE);
    }
}
