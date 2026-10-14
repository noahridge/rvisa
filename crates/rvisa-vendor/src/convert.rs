//! Mapping between rvisa types and VISA values: statuses, attributes,
//! events and timeouts (DESIGN.md V5, V6).

use std::time::Duration;

use rvisa_core::{
    Attr, AttrKind, AttrValue, Error, EventKind, FlowControl, InterfaceType, Parity, ResourceName,
    StopBits, TermReason,
};

use crate::BACKEND;
use crate::ffi::Fail;
use crate::visa::*;

/// What an error mapping needs to know about the failed call.
#[derive(Debug, Default)]
pub(crate) struct Context<'a> {
    pub resource: Option<&'a ResourceName>,
    pub attr: Option<Attr>,
    pub event: Option<EventKind>,
}

/// Maps a failed call to an `Error`. `describe` fetches `viStatusDesc`.
pub(crate) fn error(
    fail: Fail,
    ctx: &Context<'_>,
    describe: impl FnOnce(ViStatus) -> Option<String>,
) -> Error {
    let Some(status) = fail.status else {
        return Error::Unsupported {
            backend: BACKEND,
            op: fail.op,
        };
    };
    match status {
        VI_ERROR_TMO if fail.op == "viLock" => Error::ResourceLocked,
        VI_ERROR_TMO => Error::Timeout,
        VI_ERROR_RSRC_LOCKED => Error::ResourceLocked,
        VI_ERROR_RSRC_NFOUND => match ctx.resource {
            Some(rn) => Error::NotFound(rn.clone()),
            None => vendor(status, describe),
        },
        VI_ERROR_NSUP_OPER
        | VI_ERROR_NIMPL_OPER
        | VI_ERROR_NSUP_ATTR
        | VI_ERROR_NSUP_ATTR_STATE
        | VI_ERROR_NSUP_MECH => Error::Unsupported {
            backend: BACKEND,
            op: fail.op,
        },
        VI_ERROR_ATTR_READONLY => match ctx.attr {
            Some(attr) => Error::AttrReadOnly(attr),
            None => vendor(status, describe),
        },
        // Every `EventKind` maps to a valid VISA event type, so on an event
        // call this can only mean the resource lacks that event (NI-VISA on
        // SOCKET resources and SRQ). Not on viAssertTrigger: an invalid
        // setup there means the resource cannot trigger as configured
        // (NI-VISA on SOCKET resources). DESIGN.md V13.
        VI_ERROR_INV_EVENT if ctx.event.is_some() => Error::Unsupported {
            backend: BACKEND,
            op: fail.op,
        },
        VI_ERROR_INV_SETUP if fail.op == "viAssertTrigger" => Error::Unsupported {
            backend: BACKEND,
            op: fail.op,
        },
        VI_ERROR_NENABLED => match ctx.event {
            Some(kind) => Error::EventNotEnabled(kind),
            None => vendor(status, describe),
        },
        VI_ERROR_SESN_NLOCKED => Error::InvalidArgument("the session holds no lock".into()),
        VI_ERROR_INV_RSRC_NAME => Error::InvalidResource(ctx.resource.map_or_else(
            || "rejected by VISA".into(),
            |rn| format!("{rn}: rejected by VISA"),
        )),
        _ => vendor(status, describe),
    }
}

fn vendor(status: ViStatus, describe: impl FnOnce(ViStatus) -> Option<String>) -> Error {
    let name = status_name(status);
    Error::Vendor {
        status,
        name,
        desc: describe(status).unwrap_or_else(|| name.unwrap_or("unknown VISA status").to_owned()),
    }
}

/// The reason a successful `viRead` stopped (ROADMAP D11).
pub(crate) fn term_reason(status: ViStatus) -> TermReason {
    match status {
        VI_SUCCESS_TERM_CHAR => TermReason::TermChar,
        VI_SUCCESS_MAX_CNT => TermReason::MaxCount,
        _ => TermReason::End,
    }
}

/// A timeout in VISA milliseconds; `None` is infinite.
pub(crate) fn timeout_ms(timeout: Option<Duration>) -> u32 {
    match timeout {
        None => VI_TMO_INFINITE,
        Some(d) => {
            let ms = d.as_nanos().div_ceil(1_000_000);
            u32::try_from(ms)
                .unwrap_or(VI_TMO_INFINITE - 1)
                .min(VI_TMO_INFINITE - 1)
        }
    }
}

pub(crate) fn timeout_from_ms(ms: u32) -> Option<Duration> {
    (ms != VI_TMO_INFINITE).then(|| Duration::from_millis(ms.into()))
}

pub(crate) fn event_type(kind: EventKind) -> Option<ViEventType> {
    Some(match kind {
        EventKind::ServiceRequest => VI_EVENT_SERVICE_REQ,
        EventKind::Trigger => VI_EVENT_TRIG,
        EventKind::Clear => VI_EVENT_CLEAR,
        EventKind::Exception => VI_EVENT_EXCEPTION,
        EventKind::UsbInterrupt => VI_EVENT_USB_INTR,
        _ => return None,
    })
}

/// How an attribute is stored in VISA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CType {
    U8,
    U16,
    U32,
    Bool,
    Str,
}

/// The VISA ID, C type and rvisa kind of a named attribute.
pub(crate) fn attr_spec(attr: Attr) -> Option<(ViAttr, CType)> {
    Some(match attr {
        Attr::TimeoutValue => (VI_ATTR_TMO_VALUE, CType::U32),
        Attr::TermChar => (VI_ATTR_TERMCHAR, CType::U8),
        Attr::TermCharEnabled => (VI_ATTR_TERMCHAR_EN, CType::Bool),
        Attr::SendEndEnabled => (VI_ATTR_SEND_END_EN, CType::Bool),
        Attr::SuppressEndEnabled => (VI_ATTR_SUPPRESS_END_EN, CType::Bool),
        Attr::AsrlBaud => (VI_ATTR_ASRL_BAUD, CType::U32),
        Attr::AsrlDataBits => (VI_ATTR_ASRL_DATA_BITS, CType::U16),
        Attr::AsrlParity => (VI_ATTR_ASRL_PARITY, CType::U16),
        Attr::AsrlStopBits => (VI_ATTR_ASRL_STOP_BITS, CType::U16),
        Attr::AsrlFlowControl => (VI_ATTR_ASRL_FLOW_CNTRL, CType::U16),
        Attr::TcpipKeepalive => (VI_ATTR_TCPIP_KEEPALIVE, CType::Bool),
        Attr::TcpipNodelay => (VI_ATTR_TCPIP_NODELAY, CType::Bool),
        Attr::InterfaceType => (VI_ATTR_INTF_TYPE, CType::U16),
        Attr::ManufacturerName => (VI_ATTR_MANF_NAME, CType::Str),
        Attr::ModelName => (VI_ATTR_MODEL_NAME, CType::Str),
        Attr::UsbSerialNumber => (VI_ATTR_USB_SERIAL_NUM, CType::Str),
        Attr::ResourceName => (VI_ATTR_RSRC_NAME, CType::Str),
        _ => return None,
    })
}

/// The low-order bytes of a number read with `get_attr_number`.
pub(crate) fn truncate(raw: u64, ctype: CType) -> u64 {
    match ctype {
        CType::U8 => raw & 0xFF,
        CType::U16 | CType::Bool => raw & 0xFFFF,
        CType::U32 => raw & 0xFFFF_FFFF,
        CType::Str => raw,
    }
}

/// Converts a VISA number to the attribute's rvisa value.
pub(crate) fn decode(attr: Attr, raw: u64) -> Result<AttrValue, Error> {
    let bad = || Error::Protocol {
        backend: BACKEND,
        detail: format!("VISA returned {raw} for {attr:?}"),
    };
    // Each attribute was truncated to its C width first, so these fit.
    let u16v = u16::try_from(raw).map_err(|_| bad());
    Ok(match attr.kind() {
        AttrKind::Timeout => {
            AttrValue::Timeout(timeout_from_ms(u32::try_from(raw).map_err(|_| bad())?))
        }
        AttrKind::U8 => AttrValue::U8(u8::try_from(raw).map_err(|_| bad())?),
        AttrKind::U16 => AttrValue::U16(u16v?),
        AttrKind::U32 => AttrValue::U32(u32::try_from(raw).map_err(|_| bad())?),
        AttrKind::Bool => AttrValue::Bool(raw != 0),
        AttrKind::Parity => AttrValue::Parity(match u16v? {
            VI_ASRL_PAR_NONE => Parity::None,
            VI_ASRL_PAR_ODD => Parity::Odd,
            VI_ASRL_PAR_EVEN => Parity::Even,
            VI_ASRL_PAR_MARK => Parity::Mark,
            VI_ASRL_PAR_SPACE => Parity::Space,
            _ => return Err(bad()),
        }),
        AttrKind::StopBits => AttrValue::StopBits(match u16v? {
            VI_ASRL_STOP_ONE => StopBits::One,
            VI_ASRL_STOP_ONE5 => StopBits::OnePointFive,
            VI_ASRL_STOP_TWO => StopBits::Two,
            _ => return Err(bad()),
        }),
        AttrKind::FlowControl => AttrValue::FlowControl(FlowControl::from_bits_truncate(u16v?)),
        AttrKind::InterfaceType => AttrValue::InterfaceType(match u16v? {
            VI_INTF_GPIB => InterfaceType::Gpib,
            VI_INTF_VXI => InterfaceType::Vxi,
            VI_INTF_GPIB_VXI => InterfaceType::GpibVxi,
            VI_INTF_ASRL => InterfaceType::Asrl,
            VI_INTF_PXI => InterfaceType::Pxi,
            VI_INTF_TCPIP => InterfaceType::Tcpip,
            VI_INTF_USB => InterfaceType::Usb,
            _ => return Err(bad()),
        }),
        _ => AttrValue::U64(raw),
    })
}

/// Converts an rvisa value to the `ViAttrState` VISA expects.
pub(crate) fn encode(value: &AttrValue) -> Result<u64, Error> {
    Ok(match value {
        AttrValue::Bool(b) => u64::from(if *b { VI_TRUE } else { VI_FALSE }),
        AttrValue::U8(v) => (*v).into(),
        AttrValue::U16(v) => (*v).into(),
        AttrValue::U32(v) => (*v).into(),
        AttrValue::U64(v) => *v,
        AttrValue::Timeout(t) => timeout_ms(*t).into(),
        AttrValue::Parity(p) => u64::from(match p {
            Parity::None => VI_ASRL_PAR_NONE,
            Parity::Odd => VI_ASRL_PAR_ODD,
            Parity::Even => VI_ASRL_PAR_EVEN,
            Parity::Mark => VI_ASRL_PAR_MARK,
            Parity::Space => VI_ASRL_PAR_SPACE,
            _ => return Err(Error::InvalidArgument(format!("unsupported parity {p:?}"))),
        }),
        AttrValue::StopBits(s) => u64::from(match s {
            StopBits::One => VI_ASRL_STOP_ONE,
            StopBits::OnePointFive => VI_ASRL_STOP_ONE5,
            StopBits::Two => VI_ASRL_STOP_TWO,
            _ => {
                return Err(Error::InvalidArgument(format!(
                    "unsupported stop bits {s:?}"
                )));
            }
        }),
        AttrValue::FlowControl(f) => f.bits().into(),
        other => {
            return Err(Error::InvalidArgument(format!(
                "{other:?} cannot be written to a VISA attribute"
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fail(op: &'static str, status: ViStatus) -> Fail {
        Fail {
            op,
            status: Some(status),
        }
    }

    #[test]
    fn status_mapping() {
        let rn: ResourceName = "GPIB0::1".parse().unwrap();
        let ctx = Context {
            resource: Some(&rn),
            attr: Some(Attr::ModelName),
            event: Some(EventKind::ServiceRequest),
        };
        let none = |_| None;
        let map = |f| error(f, &ctx, none);
        assert!(matches!(map(fail("viRead", VI_ERROR_TMO)), Error::Timeout));
        assert!(matches!(
            map(fail("viLock", VI_ERROR_TMO)),
            Error::ResourceLocked
        ));
        assert!(matches!(
            map(fail("viRead", VI_ERROR_RSRC_LOCKED)),
            Error::ResourceLocked
        ));
        assert!(matches!(
            map(fail("viOpen", VI_ERROR_RSRC_NFOUND)),
            Error::NotFound(_)
        ));
        assert!(matches!(
            map(fail("viReadSTB", VI_ERROR_NSUP_OPER)),
            Error::Unsupported {
                op: "viReadSTB",
                ..
            }
        ));
        assert!(matches!(
            map(fail("viSetAttribute", VI_ERROR_ATTR_READONLY)),
            Error::AttrReadOnly(Attr::ModelName)
        ));
        assert!(matches!(
            map(fail("viWaitOnEvent", VI_ERROR_NENABLED)),
            Error::EventNotEnabled(EventKind::ServiceRequest)
        ));
        assert!(matches!(
            map(fail("viWaitOnEvent", VI_ERROR_INV_EVENT)),
            Error::Unsupported {
                op: "viWaitOnEvent",
                ..
            }
        ));
        assert!(matches!(
            map(fail("viAssertTrigger", VI_ERROR_INV_SETUP)),
            Error::Unsupported {
                op: "viAssertTrigger",
                ..
            }
        ));
        assert!(matches!(
            map(fail("viSetAttribute", VI_ERROR_INV_SETUP)),
            Error::Vendor { .. }
        ));
        assert!(matches!(
            map(fail("viUnlock", VI_ERROR_SESN_NLOCKED)),
            Error::InvalidArgument(_)
        ));
        assert!(matches!(
            map(fail("viOpen", VI_ERROR_INV_RSRC_NAME)),
            Error::InvalidResource(_)
        ));
        let missing = Fail {
            op: "viFlush",
            status: None,
        };
        assert!(matches!(
            map(missing),
            Error::Unsupported { op: "viFlush", .. }
        ));

        // Without context, specific statuses fall back to `Vendor`.
        let bare = Context::default();
        for status in [
            VI_ERROR_RSRC_NFOUND,
            VI_ERROR_ATTR_READONLY,
            VI_ERROR_NENABLED,
            VI_ERROR_INV_EVENT,
        ] {
            assert!(matches!(
                error(fail("x", status), &bare, none),
                Error::Vendor { .. }
            ));
        }
        assert!(matches!(
            error(fail("x", VI_ERROR_INV_RSRC_NAME), &bare, none),
            Error::InvalidResource(_)
        ));
        match error(fail("viWrite", VI_ERROR_IO), &ctx, |_| {
            Some("I/O failed".into())
        }) {
            Error::Vendor { status, name, desc } => {
                assert_eq!(status, VI_ERROR_IO);
                assert_eq!(name, Some("VI_ERROR_IO"));
                assert_eq!(desc, "I/O failed");
            }
            other => panic!("{other:?}"),
        }
        match error(fail("viWrite", -7), &ctx, none) {
            Error::Vendor {
                name: None, desc, ..
            } => assert_eq!(desc, "unknown VISA status"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reasons_and_timeouts() {
        assert_eq!(term_reason(VI_SUCCESS), TermReason::End);
        assert_eq!(term_reason(VI_SUCCESS_TERM_CHAR), TermReason::TermChar);
        assert_eq!(term_reason(VI_SUCCESS_MAX_CNT), TermReason::MaxCount);
        assert_eq!(timeout_ms(None), VI_TMO_INFINITE);
        assert_eq!(timeout_ms(Some(Duration::from_micros(1500))), 2);
        assert_eq!(
            timeout_ms(Some(Duration::from_secs(u64::MAX))),
            VI_TMO_INFINITE - 1
        );
        assert_eq!(timeout_from_ms(VI_TMO_INFINITE), None);
        assert_eq!(timeout_from_ms(5), Some(Duration::from_millis(5)));
    }

    #[test]
    fn events() {
        for kind in [
            EventKind::ServiceRequest,
            EventKind::Trigger,
            EventKind::Clear,
            EventKind::Exception,
            EventKind::UsbInterrupt,
        ] {
            assert!(event_type(kind).is_some());
        }
    }

    #[test]
    fn attribute_values_round_trip() {
        let cases = [
            (
                Attr::TimeoutValue,
                AttrValue::Timeout(Some(Duration::from_millis(2500))),
            ),
            (Attr::TimeoutValue, AttrValue::Timeout(None)),
            (Attr::TermChar, AttrValue::U8(b'\r')),
            (Attr::TermCharEnabled, AttrValue::Bool(true)),
            (Attr::AsrlBaud, AttrValue::U32(115_200)),
            (Attr::AsrlDataBits, AttrValue::U16(7)),
            (
                Attr::AsrlFlowControl,
                AttrValue::FlowControl(FlowControl::RTS_CTS),
            ),
        ];
        for (attr, value) in cases {
            let (_, ctype) = attr_spec(attr).unwrap();
            let raw = truncate(encode(&value).unwrap(), ctype);
            assert_eq!(decode(attr, raw).unwrap(), value, "{attr:?}");
        }
        for p in [
            Parity::None,
            Parity::Odd,
            Parity::Even,
            Parity::Mark,
            Parity::Space,
        ] {
            let raw = encode(&AttrValue::Parity(p)).unwrap();
            assert_eq!(decode(Attr::AsrlParity, raw).unwrap(), AttrValue::Parity(p));
        }
        for s in [StopBits::One, StopBits::OnePointFive, StopBits::Two] {
            let raw = encode(&AttrValue::StopBits(s)).unwrap();
            assert_eq!(
                decode(Attr::AsrlStopBits, raw).unwrap(),
                AttrValue::StopBits(s)
            );
        }
        for (raw, intf) in [
            (1, InterfaceType::Gpib),
            (2, InterfaceType::Vxi),
            (3, InterfaceType::GpibVxi),
            (4, InterfaceType::Asrl),
            (5, InterfaceType::Pxi),
            (6, InterfaceType::Tcpip),
            (7, InterfaceType::Usb),
        ] {
            assert_eq!(
                decode(Attr::InterfaceType, raw).unwrap(),
                AttrValue::InterfaceType(intf)
            );
        }
        assert!(decode(Attr::InterfaceType, 99).is_err());
        assert!(decode(Attr::AsrlParity, 99).is_err());
        assert!(decode(Attr::AsrlStopBits, 99).is_err());
        assert!(decode(Attr::TermChar, 300).is_err());
        assert_eq!(decode(Attr::Raw(1), 7).unwrap(), AttrValue::U64(7));
        assert!(encode(&AttrValue::Str("x".into())).is_err());
        assert_eq!(truncate(0x1_2345, CType::U8), 0x45);
        assert_eq!(truncate(0x1_2345, CType::Str), 0x1_2345);
        assert!(attr_spec(Attr::Raw(1)).is_none());
    }
}
