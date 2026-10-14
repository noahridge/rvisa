//! VISA C types and constants, hand-written from IVI VPP-4.3 (D3).
//!
//! Only what the vendor backend uses is declared. The values follow the
//! specification and were checked against NI-VISA 26.5's `visa.h` on
//! Windows (see DESIGN.md V5).

#![allow(missing_docs)] // The names are the VISA specification's.

use std::ffi::c_char;

pub type ViUInt32 = u32;
pub type ViInt32 = i32;
pub type ViUInt16 = u16;
pub type ViInt16 = i16;
pub type ViUInt8 = u8;
pub type ViChar = c_char;
pub type ViBoolean = ViUInt16;
pub type ViStatus = ViInt32;
pub type ViObject = ViUInt32;
pub type ViSession = ViObject;
pub type ViFindList = ViObject;
pub type ViEvent = ViObject;
pub type ViAttr = ViUInt32;
pub type ViAccessMode = ViUInt32;
pub type ViEventType = ViUInt32;
pub type ViEventFilter = ViUInt32;

/// `ViAttrState` is pointer-sized in `visatype.h`.
#[cfg(target_pointer_width = "64")]
pub type ViAttrState = u64;
/// `ViAttrState` is pointer-sized in `visatype.h`.
#[cfg(not(target_pointer_width = "64"))]
pub type ViAttrState = u32;

pub const VI_NULL: ViUInt32 = 0;
pub const VI_TRUE: ViBoolean = 1;
pub const VI_FALSE: ViBoolean = 0;
/// Minimum length of resource-name, description and key buffers.
pub const VI_FIND_BUFLEN: usize = 256;

pub const VI_NO_LOCK: ViAccessMode = 0;
pub const VI_EXCLUSIVE_LOCK: ViAccessMode = 1;
pub const VI_SHARED_LOCK: ViAccessMode = 2;

pub const VI_TMO_IMMEDIATE: ViUInt32 = 0;
pub const VI_TMO_INFINITE: ViUInt32 = 0xFFFF_FFFF;

pub const VI_QUEUE: ViUInt16 = 1;

pub const VI_TRIG_PROT_DEFAULT: ViUInt16 = 0;

pub const VI_READ_BUF: ViUInt16 = 1;
pub const VI_WRITE_BUF: ViUInt16 = 2;
pub const VI_READ_BUF_DISCARD: ViUInt16 = 4;
pub const VI_WRITE_BUF_DISCARD: ViUInt16 = 8;
pub const VI_IO_IN_BUF: ViUInt16 = 16;
pub const VI_IO_OUT_BUF: ViUInt16 = 32;
pub const VI_IO_IN_BUF_DISCARD: ViUInt16 = 64;
pub const VI_IO_OUT_BUF_DISCARD: ViUInt16 = 128;

pub const VI_GPIB_REN_DEASSERT: ViUInt16 = 0;
pub const VI_GPIB_REN_ASSERT: ViUInt16 = 1;
pub const VI_GPIB_REN_DEASSERT_GTL: ViUInt16 = 2;
pub const VI_GPIB_REN_ASSERT_ADDRESS: ViUInt16 = 3;
pub const VI_GPIB_REN_ASSERT_LLO: ViUInt16 = 4;
pub const VI_GPIB_REN_ASSERT_ADDRESS_LLO: ViUInt16 = 5;
pub const VI_GPIB_REN_ADDRESS_GTL: ViUInt16 = 6;

// Interface types (VI_ATTR_INTF_TYPE)
pub const VI_INTF_GPIB: ViUInt16 = 1;
pub const VI_INTF_VXI: ViUInt16 = 2;
pub const VI_INTF_GPIB_VXI: ViUInt16 = 3;
pub const VI_INTF_ASRL: ViUInt16 = 4;
pub const VI_INTF_PXI: ViUInt16 = 5;
pub const VI_INTF_TCPIP: ViUInt16 = 6;
pub const VI_INTF_USB: ViUInt16 = 7;

// Serial settings
pub const VI_ASRL_PAR_NONE: ViUInt16 = 0;
pub const VI_ASRL_PAR_ODD: ViUInt16 = 1;
pub const VI_ASRL_PAR_EVEN: ViUInt16 = 2;
pub const VI_ASRL_PAR_MARK: ViUInt16 = 3;
pub const VI_ASRL_PAR_SPACE: ViUInt16 = 4;
pub const VI_ASRL_STOP_ONE: ViUInt16 = 10;
pub const VI_ASRL_STOP_ONE5: ViUInt16 = 15;
pub const VI_ASRL_STOP_TWO: ViUInt16 = 20;

// Events
pub const VI_EVENT_IO_COMPLETION: ViEventType = 0x3FFF_2009;
pub const VI_EVENT_TRIG: ViEventType = 0xBFFF_200A;
pub const VI_EVENT_SERVICE_REQ: ViEventType = 0x3FFF_200B;
pub const VI_EVENT_CLEAR: ViEventType = 0x3FFF_200D;
pub const VI_EVENT_EXCEPTION: ViEventType = 0xBFFF_200E;
pub const VI_EVENT_USB_INTR: ViEventType = 0x3FFF_2037;

// Attributes
pub const VI_ATTR_RSRC_NAME: ViAttr = 0xBFFF_0002;
pub const VI_ATTR_TMO_VALUE: ViAttr = 0x3FFF_001A;
pub const VI_ATTR_TERMCHAR: ViAttr = 0x3FFF_0018;
pub const VI_ATTR_TERMCHAR_EN: ViAttr = 0x3FFF_0038;
pub const VI_ATTR_SEND_END_EN: ViAttr = 0x3FFF_0016;
pub const VI_ATTR_SUPPRESS_END_EN: ViAttr = 0x3FFF_0036;
pub const VI_ATTR_ASRL_BAUD: ViAttr = 0x3FFF_0021;
pub const VI_ATTR_ASRL_DATA_BITS: ViAttr = 0x3FFF_0022;
pub const VI_ATTR_ASRL_PARITY: ViAttr = 0x3FFF_0023;
pub const VI_ATTR_ASRL_STOP_BITS: ViAttr = 0x3FFF_0024;
pub const VI_ATTR_ASRL_FLOW_CNTRL: ViAttr = 0x3FFF_0025;
pub const VI_ATTR_TCPIP_NODELAY: ViAttr = 0x3FFF_019A;
pub const VI_ATTR_TCPIP_KEEPALIVE: ViAttr = 0x3FFF_019B;
pub const VI_ATTR_INTF_TYPE: ViAttr = 0x3FFF_0171;
pub const VI_ATTR_MANF_NAME: ViAttr = 0xBFFF_0072;
pub const VI_ATTR_MODEL_NAME: ViAttr = 0xBFFF_0077;
pub const VI_ATTR_USB_SERIAL_NUM: ViAttr = 0xBFFF_01A0;

/// Whether a raw attribute ID denotes a string attribute (`0xBFFF` prefix).
#[must_use]
pub fn is_string_attr(attr: ViAttr) -> bool {
    attr >> 16 == 0xBFFF
}

macro_rules! statuses {
    ($($name:ident = $value:literal,)*) => {
        $(pub const $name: ViStatus = $value as u32 as ViStatus;)*

        /// The symbolic name of a status code, such as `"VI_ERROR_TMO"`.
        #[must_use]
        pub fn status_name(status: ViStatus) -> Option<&'static str> {
            match status {
                $($name => Some(stringify!($name)),)*
                _ => None,
            }
        }

        /// Every status code in the table.
        pub const ALL_STATUSES: &[ViStatus] = &[$($name,)*];
    };
}

statuses! {
    VI_SUCCESS = 0x0000_0000,
    VI_SUCCESS_EVENT_EN = 0x3FFF_0002,
    VI_SUCCESS_EVENT_DIS = 0x3FFF_0003,
    VI_SUCCESS_QUEUE_EMPTY = 0x3FFF_0004,
    VI_SUCCESS_TERM_CHAR = 0x3FFF_0005,
    VI_SUCCESS_MAX_CNT = 0x3FFF_0006,
    VI_SUCCESS_DEV_NPRESENT = 0x3FFF_007D,
    VI_SUCCESS_TRIG_MAPPED = 0x3FFF_007E,
    VI_SUCCESS_QUEUE_NEMPTY = 0x3FFF_0080,
    VI_SUCCESS_NCHAIN = 0x3FFF_0098,
    VI_SUCCESS_NESTED_SHARED = 0x3FFF_0099,
    VI_SUCCESS_NESTED_EXCLUSIVE = 0x3FFF_009A,
    VI_SUCCESS_SYNC = 0x3FFF_009B,

    VI_WARN_QUEUE_OVERFLOW = 0x3FFF_000C,
    VI_WARN_CONFIG_NLOADED = 0x3FFF_0077,
    VI_WARN_NULL_OBJECT = 0x3FFF_0082,
    VI_WARN_NSUP_ATTR_STATE = 0x3FFF_0084,
    VI_WARN_UNKNOWN_STATUS = 0x3FFF_0085,
    VI_WARN_NSUP_BUF = 0x3FFF_0088,
    VI_WARN_EXT_FUNC_NIMPL = 0x3FFF_00A9,

    VI_ERROR_SYSTEM_ERROR = 0xBFFF_0000,
    VI_ERROR_INV_OBJECT = 0xBFFF_000E,
    VI_ERROR_RSRC_LOCKED = 0xBFFF_000F,
    VI_ERROR_INV_EXPR = 0xBFFF_0010,
    VI_ERROR_RSRC_NFOUND = 0xBFFF_0011,
    VI_ERROR_INV_RSRC_NAME = 0xBFFF_0012,
    VI_ERROR_INV_ACC_MODE = 0xBFFF_0013,
    VI_ERROR_TMO = 0xBFFF_0015,
    VI_ERROR_CLOSING_FAILED = 0xBFFF_0016,
    VI_ERROR_INV_DEGREE = 0xBFFF_001B,
    VI_ERROR_INV_JOB_ID = 0xBFFF_001C,
    VI_ERROR_NSUP_ATTR = 0xBFFF_001D,
    VI_ERROR_NSUP_ATTR_STATE = 0xBFFF_001E,
    VI_ERROR_ATTR_READONLY = 0xBFFF_001F,
    VI_ERROR_INV_LOCK_TYPE = 0xBFFF_0020,
    VI_ERROR_INV_ACCESS_KEY = 0xBFFF_0021,
    VI_ERROR_INV_EVENT = 0xBFFF_0026,
    VI_ERROR_INV_MECH = 0xBFFF_0027,
    VI_ERROR_HNDLR_NINSTALLED = 0xBFFF_0028,
    VI_ERROR_INV_HNDLR_REF = 0xBFFF_0029,
    VI_ERROR_INV_CONTEXT = 0xBFFF_002A,
    VI_ERROR_QUEUE_OVERFLOW = 0xBFFF_002D,
    VI_ERROR_NENABLED = 0xBFFF_002F,
    VI_ERROR_ABORT = 0xBFFF_0030,
    VI_ERROR_RAW_WR_PROT_VIOL = 0xBFFF_0034,
    VI_ERROR_RAW_RD_PROT_VIOL = 0xBFFF_0035,
    VI_ERROR_OUTP_PROT_VIOL = 0xBFFF_0036,
    VI_ERROR_INP_PROT_VIOL = 0xBFFF_0037,
    VI_ERROR_BERR = 0xBFFF_0038,
    VI_ERROR_IN_PROGRESS = 0xBFFF_0039,
    VI_ERROR_INV_SETUP = 0xBFFF_003A,
    VI_ERROR_QUEUE_ERROR = 0xBFFF_003B,
    VI_ERROR_ALLOC = 0xBFFF_003C,
    VI_ERROR_INV_MASK = 0xBFFF_003D,
    VI_ERROR_IO = 0xBFFF_003E,
    VI_ERROR_INV_FMT = 0xBFFF_003F,
    VI_ERROR_NSUP_FMT = 0xBFFF_0041,
    VI_ERROR_LINE_IN_USE = 0xBFFF_0042,
    VI_ERROR_LINE_NRESERVED = 0xBFFF_0043,
    VI_ERROR_NSUP_MODE = 0xBFFF_0046,
    VI_ERROR_SRQ_NOCCURRED = 0xBFFF_004A,
    VI_ERROR_INV_SPACE = 0xBFFF_004E,
    VI_ERROR_INV_OFFSET = 0xBFFF_0051,
    VI_ERROR_INV_WIDTH = 0xBFFF_0052,
    VI_ERROR_NSUP_OFFSET = 0xBFFF_0054,
    VI_ERROR_NSUP_VAR_WIDTH = 0xBFFF_0055,
    VI_ERROR_WINDOW_NMAPPED = 0xBFFF_0057,
    VI_ERROR_RESP_PENDING = 0xBFFF_0059,
    VI_ERROR_NLISTENERS = 0xBFFF_005F,
    VI_ERROR_NCIC = 0xBFFF_0060,
    VI_ERROR_NSYS_CNTLR = 0xBFFF_0061,
    VI_ERROR_NSUP_OPER = 0xBFFF_0067,
    VI_ERROR_INTR_PENDING = 0xBFFF_0068,
    VI_ERROR_ASRL_PARITY = 0xBFFF_006A,
    VI_ERROR_ASRL_FRAMING = 0xBFFF_006B,
    VI_ERROR_ASRL_OVERRUN = 0xBFFF_006C,
    VI_ERROR_TRIG_NMAPPED = 0xBFFF_006E,
    VI_ERROR_NSUP_ALIGN_OFFSET = 0xBFFF_0070,
    VI_ERROR_USER_BUF = 0xBFFF_0071,
    VI_ERROR_RSRC_BUSY = 0xBFFF_0072,
    VI_ERROR_NSUP_WIDTH = 0xBFFF_0076,
    VI_ERROR_INV_PARAMETER = 0xBFFF_0078,
    VI_ERROR_INV_PROT = 0xBFFF_0079,
    VI_ERROR_INV_SIZE = 0xBFFF_007B,
    VI_ERROR_WINDOW_MAPPED = 0xBFFF_0080,
    VI_ERROR_NIMPL_OPER = 0xBFFF_0081,
    VI_ERROR_INV_LENGTH = 0xBFFF_0083,
    VI_ERROR_INV_MODE = 0xBFFF_0091,
    VI_ERROR_SESN_NLOCKED = 0xBFFF_009C,
    VI_ERROR_MEM_NSHARED = 0xBFFF_009D,
    VI_ERROR_LIBRARY_NFOUND = 0xBFFF_009E,
    VI_ERROR_NSUP_INTR = 0xBFFF_009F,
    VI_ERROR_INV_LINE = 0xBFFF_00A0,
    VI_ERROR_FILE_ACCESS = 0xBFFF_00A1,
    VI_ERROR_FILE_IO = 0xBFFF_00A2,
    VI_ERROR_NSUP_LINE = 0xBFFF_00A3,
    VI_ERROR_NSUP_MECH = 0xBFFF_00A4,
    VI_ERROR_INTF_NUM_NCONFIG = 0xBFFF_00A5,
    VI_ERROR_CONN_LOST = 0xBFFF_00A6,
    VI_ERROR_MACHINE_NAVAIL = 0xBFFF_00A7,
    VI_ERROR_NPERMISSION = 0xBFFF_00A8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_table() {
        let mut codes = ALL_STATUSES.to_vec();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ALL_STATUSES.len(), "duplicate status codes");
        for &status in ALL_STATUSES {
            let name = status_name(status).unwrap();
            assert_eq!(status < 0, name.starts_with("VI_ERROR_"), "{name}");
        }
        assert_eq!(VI_ERROR_TMO, 0xBFFF_0015_u32 as i32);
        assert_eq!(status_name(12345), None);
    }

    #[test]
    fn string_attrs() {
        assert!(is_string_attr(VI_ATTR_RSRC_NAME));
        assert!(!is_string_attr(VI_ATTR_TMO_VALUE));
    }
}
