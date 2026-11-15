//! VXI-11 messages: program numbers, procedures, flags, error codes and
//! the argument and result structures (VXI-11 TCP/IP Instrument Protocol
//! Specification, rev. 1.0, appendix B).

// The server-side codecs serve the `server` and `srq` features.
#![cfg_attr(not(all(feature = "server", feature = "srq")), allow(dead_code))]

use std::time::Duration;

use rvisa_core::{Error, Result};

use crate::BACKEND;
use crate::xdr::{Decoder, Encoder};

/// The portmapper (RFC 1833, version 2).
pub(crate) const PORTMAP_PROG: u32 = 100_000;
pub(crate) const PORTMAP_VERS: u32 = 2;
pub(crate) const PMAPPROC_GETPORT: u32 = 3;
pub(crate) const IPPROTO_TCP: u32 = 6;
pub(crate) const PORTMAP_PORT: u16 = 111;

pub(crate) const DEVICE_CORE: u32 = 0x0607AF;
pub(crate) const DEVICE_ASYNC: u32 = 0x0607B0;
pub(crate) const DEVICE_INTR: u32 = 0x0607B1;
pub(crate) const DEVICE_VERSION: u32 = 1;

// Core channel procedures.
pub(crate) const CREATE_LINK: u32 = 10;
pub(crate) const DEVICE_WRITE: u32 = 11;
pub(crate) const DEVICE_READ: u32 = 12;
pub(crate) const DEVICE_READSTB: u32 = 13;
pub(crate) const DEVICE_TRIGGER: u32 = 14;
pub(crate) const DEVICE_CLEAR: u32 = 15;
pub(crate) const DEVICE_REMOTE: u32 = 16;
pub(crate) const DEVICE_LOCAL: u32 = 17;
pub(crate) const DEVICE_LOCK: u32 = 18;
pub(crate) const DEVICE_UNLOCK: u32 = 19;
pub(crate) const DEVICE_ENABLE_SRQ: u32 = 20;
pub(crate) const DESTROY_LINK: u32 = 23;
pub(crate) const CREATE_INTR_CHAN: u32 = 25;
pub(crate) const DESTROY_INTR_CHAN: u32 = 26;
// Abort channel.
pub(crate) const DEVICE_ABORT: u32 = 1;
// Interrupt channel (the client serves it).
pub(crate) const DEVICE_INTR_SRQ: u32 = 30;

// Device_Flags.
pub(crate) const FLAG_WAITLOCK: u32 = 0x01;
pub(crate) const FLAG_END: u32 = 0x08;
pub(crate) const FLAG_TERMCHRSET: u32 = 0x80;

// device_read reasons.
pub(crate) const REASON_REQCNT: u32 = 0x01;
pub(crate) const REASON_CHR: u32 = 0x02;
pub(crate) const REASON_END: u32 = 0x04;

/// The longest handle `device_enable_srq` and `device_intr_srq` carry.
pub(crate) const MAX_SRQ_HANDLE: usize = 40;
/// The longest device name or string this crate accepts from a peer.
pub(crate) const MAX_NAME: usize = 1024;

/// VXI-11 `Device_ErrorCode` values (spec table B.2).
pub(crate) mod code {
    pub const NO_ERROR: i32 = 0;
    pub const SYNTAX_ERROR: i32 = 1;
    pub const NOT_ACCESSIBLE: i32 = 3;
    pub const INVALID_LINK: i32 = 4;
    pub const PARAMETER_ERROR: i32 = 5;
    pub const NO_CHANNEL: i32 = 6;
    pub const NOT_SUPPORTED: i32 = 8;
    pub const OUT_OF_RESOURCES: i32 = 9;
    pub const LOCKED: i32 = 11;
    pub const NO_LOCK_HELD: i32 = 12;
    pub const IO_TIMEOUT: i32 = 15;
    pub const IO_ERROR: i32 = 17;
    pub const INVALID_ADDRESS: i32 = 21;
    pub const ABORT: i32 = 23;
    pub const CHANNEL_ESTABLISHED: i32 = 29;
}

/// Converts a VXI-11 error code to an [`Error`]. `op` names the procedure
/// (and `Unsupported` carries it).
pub(crate) fn check(code: i32, op: &'static str) -> Result<()> {
    let detail = match code {
        code::NO_ERROR => return Ok(()),
        code::LOCKED => return Err(Error::ResourceLocked),
        code::IO_TIMEOUT => return Err(Error::timeout()),
        code::NOT_SUPPORTED => {
            return Err(Error::Unsupported {
                backend: BACKEND,
                op,
            });
        }
        code::NO_LOCK_HELD => {
            return Err(Error::InvalidArgument("the session holds no lock".into()));
        }
        code::SYNTAX_ERROR => "syntax error",
        code::NOT_ACCESSIBLE => "device not accessible",
        code::INVALID_LINK => "invalid link identifier",
        code::PARAMETER_ERROR => "parameter error",
        code::NO_CHANNEL => "channel not established",
        code::OUT_OF_RESOURCES => "out of resources",
        code::IO_ERROR => "I/O error",
        code::INVALID_ADDRESS => "invalid address",
        code::ABORT => "aborted",
        code::CHANNEL_ESTABLISHED => "channel already established",
        _ => "unknown error",
    };
    Err(Error::Protocol {
        backend: BACKEND,
        detail: format!("{op}: device error {code} ({detail})"),
    })
}

/// A VXI-11 timeout in milliseconds. `None` (infinite) is the largest
/// value, as NI-VISA sends it.
pub(crate) fn millis(timeout: Option<Duration>) -> u32 {
    timeout.map_or(u32::MAX, |t| {
        u32::try_from(t.as_millis()).unwrap_or(u32::MAX - 1)
    })
}

/// `Create_LinkParms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreateLinkParms {
    pub client_id: i32,
    pub lock_device: bool,
    pub lock_timeout: u32,
    pub device: String,
}

impl CreateLinkParms {
    pub fn encode(&self) -> Vec<u8> {
        Encoder::new()
            .i32(self.client_id)
            .bool(self.lock_device)
            .u32(self.lock_timeout)
            .string(&self.device)
            .finish()
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut d = Decoder::new(data);
        let parms = Self {
            client_id: d.i32()?,
            lock_device: d.bool()?,
            lock_timeout: d.u32()?,
            device: d.string(MAX_NAME)?,
        };
        d.finish()?;
        Ok(parms)
    }
}

/// `Create_LinkResp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreateLinkResp {
    pub error: i32,
    pub lid: i32,
    pub abort_port: u16,
    pub max_recv_size: u32,
}

impl CreateLinkResp {
    pub fn encode(&self) -> Vec<u8> {
        Encoder::new()
            .i32(self.error)
            .i32(self.lid)
            .u32(u32::from(self.abort_port))
            .u32(self.max_recv_size)
            .finish()
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut d = Decoder::new(data);
        let error = d.i32()?;
        let lid = d.i32()?;
        // An unsigned short, sent as a 32-bit integer.
        let abort_port = (d.u32()? & 0xFFFF) as u16;
        let max_recv_size = d.u32()?;
        d.finish()?;
        Ok(Self {
            error,
            lid,
            abort_port,
            max_recv_size,
        })
    }
}

/// `Device_WriteParms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteParms<'a> {
    pub lid: i32,
    pub io_timeout: u32,
    pub lock_timeout: u32,
    pub flags: u32,
    pub data: &'a [u8],
}

impl<'a> WriteParms<'a> {
    pub fn encode(&self) -> Vec<u8> {
        Encoder::new()
            .i32(self.lid)
            .u32(self.io_timeout)
            .u32(self.lock_timeout)
            .u32(self.flags)
            .opaque(self.data)
            .finish()
    }

    pub fn decode(data: &'a [u8], max: usize) -> Result<Self> {
        let mut d = Decoder::new(data);
        let parms = Self {
            lid: d.i32()?,
            io_timeout: d.u32()?,
            lock_timeout: d.u32()?,
            flags: d.u32()?,
            data: d.opaque(max)?,
        };
        d.finish()?;
        Ok(parms)
    }
}

/// `Device_WriteResp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteResp {
    pub error: i32,
    pub size: u32,
}

impl WriteResp {
    pub fn encode(&self) -> Vec<u8> {
        Encoder::new().i32(self.error).u32(self.size).finish()
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut d = Decoder::new(data);
        let resp = Self {
            error: d.i32()?,
            size: d.u32()?,
        };
        d.finish()?;
        Ok(resp)
    }
}

/// `Device_ReadParms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadParms {
    pub lid: i32,
    pub request_size: u32,
    pub io_timeout: u32,
    pub lock_timeout: u32,
    pub flags: u32,
    pub term_char: u8,
}

impl ReadParms {
    pub fn encode(&self) -> Vec<u8> {
        Encoder::new()
            .i32(self.lid)
            .u32(self.request_size)
            .u32(self.io_timeout)
            .u32(self.lock_timeout)
            .u32(self.flags)
            .u32(u32::from(self.term_char))
            .finish()
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut d = Decoder::new(data);
        let parms = Self {
            lid: d.i32()?,
            request_size: d.u32()?,
            io_timeout: d.u32()?,
            lock_timeout: d.u32()?,
            flags: d.u32()?,
            term_char: (d.u32()? & 0xFF) as u8,
        };
        d.finish()?;
        Ok(parms)
    }
}

/// `Device_ReadResp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadResp {
    pub error: i32,
    pub reason: u32,
    pub data: Vec<u8>,
}

impl ReadResp {
    pub fn encode(&self) -> Vec<u8> {
        Encoder::new()
            .i32(self.error)
            .u32(self.reason)
            .opaque(&self.data)
            .finish()
    }

    pub fn decode(data: &[u8], max: usize) -> Result<Self> {
        let mut d = Decoder::new(data);
        let resp = Self {
            error: d.i32()?,
            reason: d.u32()?,
            data: d.opaque(max)?.to_vec(),
        };
        d.finish()?;
        Ok(resp)
    }
}

/// `Device_GenericParms`: for readstb, trigger, clear, remote and local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GenericParms {
    pub lid: i32,
    pub flags: u32,
    pub lock_timeout: u32,
    pub io_timeout: u32,
}

impl GenericParms {
    pub fn encode(&self) -> Vec<u8> {
        Encoder::new()
            .i32(self.lid)
            .u32(self.flags)
            .u32(self.lock_timeout)
            .u32(self.io_timeout)
            .finish()
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut d = Decoder::new(data);
        let parms = Self {
            lid: d.i32()?,
            flags: d.u32()?,
            lock_timeout: d.u32()?,
            io_timeout: d.u32()?,
        };
        d.finish()?;
        Ok(parms)
    }
}

/// `Device_ReadStbResp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadStbResp {
    pub error: i32,
    pub stb: u8,
}

impl ReadStbResp {
    pub fn encode(&self) -> Vec<u8> {
        Encoder::new()
            .i32(self.error)
            .u32(u32::from(self.stb))
            .finish()
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut d = Decoder::new(data);
        let resp = Self {
            error: d.i32()?,
            // An unsigned char, sent as a 32-bit integer.
            stb: (d.u32()? & 0xFF) as u8,
        };
        d.finish()?;
        Ok(resp)
    }
}

/// `Device_LockParms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockParms {
    pub lid: i32,
    pub flags: u32,
    pub lock_timeout: u32,
}

impl LockParms {
    pub fn encode(&self) -> Vec<u8> {
        Encoder::new()
            .i32(self.lid)
            .u32(self.flags)
            .u32(self.lock_timeout)
            .finish()
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut d = Decoder::new(data);
        let parms = Self {
            lid: d.i32()?,
            flags: d.u32()?,
            lock_timeout: d.u32()?,
        };
        d.finish()?;
        Ok(parms)
    }
}

/// `Device_EnableSrqParms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnableSrqParms {
    pub lid: i32,
    pub enable: bool,
    pub handle: Vec<u8>,
}

impl EnableSrqParms {
    pub fn encode(&self) -> Vec<u8> {
        Encoder::new()
            .i32(self.lid)
            .bool(self.enable)
            .opaque(&self.handle)
            .finish()
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut d = Decoder::new(data);
        let parms = Self {
            lid: d.i32()?,
            enable: d.bool()?,
            handle: d.opaque(MAX_SRQ_HANDLE)?.to_vec(),
        };
        d.finish()?;
        Ok(parms)
    }
}

/// `Device_RemoteFunc`: where the device sends interrupts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteFunc {
    pub host_addr: u32,
    pub host_port: u16,
    pub prog_num: u32,
    pub prog_vers: u32,
    /// `DEVICE_TCP` (0) or `DEVICE_UDP` (1).
    pub prog_family: u32,
}

impl RemoteFunc {
    pub fn encode(&self) -> Vec<u8> {
        Encoder::new()
            .u32(self.host_addr)
            .u32(u32::from(self.host_port))
            .u32(self.prog_num)
            .u32(self.prog_vers)
            .u32(self.prog_family)
            .finish()
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut d = Decoder::new(data);
        let parms = Self {
            host_addr: d.u32()?,
            host_port: (d.u32()? & 0xFFFF) as u16,
            prog_num: d.u32()?,
            prog_vers: d.u32()?,
            prog_family: d.u32()?,
        };
        d.finish()?;
        Ok(parms)
    }
}

/// `Device_SrqParms`: the argument of `device_intr_srq`.
pub(crate) fn decode_srq_handle(data: &[u8]) -> Result<Vec<u8>> {
    let mut d = Decoder::new(data);
    let handle = d.opaque(MAX_SRQ_HANDLE)?.to_vec();
    d.finish()?;
    Ok(handle)
}

pub(crate) fn encode_srq_handle(handle: &[u8]) -> Vec<u8> {
    Encoder::new().opaque(handle).finish()
}

/// `Device_Error`: the result of most procedures.
pub(crate) fn decode_error(data: &[u8]) -> Result<i32> {
    let mut d = Decoder::new(data);
    let error = d.i32()?;
    d.finish()?;
    Ok(error)
}

pub(crate) fn encode_error(error: i32) -> Vec<u8> {
    Encoder::new().i32(error).finish()
}

/// `Device_Link`: the argument of `destroy_link` and `device_abort`.
pub(crate) fn encode_link(lid: i32) -> Vec<u8> {
    Encoder::new().i32(lid).finish()
}

pub(crate) fn decode_link(data: &[u8]) -> Result<i32> {
    decode_error(data)
}

/// Portmapper `GETPORT` arguments (`struct mapping`).
pub(crate) fn encode_getport(prog: u32, vers: u32) -> Vec<u8> {
    Encoder::new()
        .u32(prog)
        .u32(vers)
        .u32(IPPROTO_TCP)
        .u32(0)
        .finish()
}

/// Decodes `GETPORT` arguments: program, version and protocol.
pub(crate) fn decode_getport(data: &[u8]) -> Result<(u32, u32, u32)> {
    let mut d = Decoder::new(data);
    let mapping = (d.u32()?, d.u32()?, d.u32()?);
    d.u32()?;
    d.finish()?;
    Ok(mapping)
}

/// The `GETPORT` result: a port, 0 if the program is not registered.
pub(crate) fn decode_port(data: &[u8]) -> Result<u16> {
    let mut d = Decoder::new(data);
    let port = d.u32()?;
    d.finish()?;
    u16::try_from(port).map_err(|_| crate::xdr::malformed(format!("port {port}")))
}

pub(crate) fn encode_port(port: u16) -> Vec<u8> {
    Encoder::new().u32(u32::from(port)).finish()
}

/// Decodes every reply and argument structure, for the fuzzer: none may
/// panic, whatever the input.
pub(crate) fn decode_any(data: &[u8]) {
    let _ = CreateLinkParms::decode(data);
    let _ = CreateLinkResp::decode(data);
    let _ = WriteParms::decode(data, usize::MAX);
    let _ = WriteResp::decode(data);
    let _ = ReadParms::decode(data);
    let _ = ReadResp::decode(data, usize::MAX);
    let _ = GenericParms::decode(data);
    let _ = ReadStbResp::decode(data);
    let _ = LockParms::decode(data);
    let _ = EnableSrqParms::decode(data);
    let _ = RemoteFunc::decode(data);
    let _ = decode_srq_handle(data);
    let _ = decode_error(data);
    let _ = decode_getport(data);
    let _ = decode_port(data);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structures_roundtrip() {
        let link = CreateLinkParms {
            client_id: 3,
            lock_device: false,
            lock_timeout: 0,
            device: "gpib0,5".into(),
        };
        assert_eq!(CreateLinkParms::decode(&link.encode()).unwrap(), link);
        let resp = CreateLinkResp {
            error: 0,
            lid: 9,
            abort_port: 1024,
            max_recv_size: 0x10000,
        };
        assert_eq!(CreateLinkResp::decode(&resp.encode()).unwrap(), resp);
        let write = WriteParms {
            lid: 9,
            io_timeout: 2000,
            lock_timeout: 0,
            flags: FLAG_END,
            data: b"*IDN?\n",
        };
        let bytes = write.encode();
        assert_eq!(WriteParms::decode(&bytes, 100).unwrap(), write);
        assert!(WriteParms::decode(&bytes, 2).is_err(), "over the limit");
        let read = ReadParms {
            lid: 9,
            request_size: 512,
            io_timeout: 1,
            lock_timeout: 0,
            flags: FLAG_TERMCHRSET,
            term_char: b'\n',
        };
        assert_eq!(ReadParms::decode(&read.encode()).unwrap(), read);
        let data = ReadResp {
            error: 0,
            reason: REASON_END,
            data: b"x".to_vec(),
        };
        assert_eq!(ReadResp::decode(&data.encode(), 1).unwrap(), data);
        let srq = EnableSrqParms {
            lid: 1,
            enable: true,
            handle: b"h".to_vec(),
        };
        assert_eq!(EnableSrqParms::decode(&srq.encode()).unwrap(), srq);
        let func = RemoteFunc {
            host_addr: 0x7F00_0001,
            host_port: 4000,
            prog_num: DEVICE_INTR,
            prog_vers: 1,
            prog_family: 0,
        };
        assert_eq!(RemoteFunc::decode(&func.encode()).unwrap(), func);
        assert_eq!(
            decode_srq_handle(&encode_srq_handle(b"abc")).unwrap(),
            b"abc"
        );
        assert_eq!(
            decode_getport(&encode_getport(DEVICE_CORE, 1)).unwrap(),
            (DEVICE_CORE, 1, IPPROTO_TCP)
        );
        assert_eq!(decode_port(&encode_port(5025)).unwrap(), 5025);
        assert!(decode_port(&[0, 1, 0, 0]).is_err());
        assert!(decode_error(&[0, 0, 0, 0, 0]).is_err(), "trailing byte");
    }

    #[test]
    fn error_codes() {
        assert!(check(0, "x").is_ok());
        assert!(matches!(check(11, "x"), Err(Error::ResourceLocked)));
        assert!(check(15, "x").unwrap_err().is_timeout());
        assert!(matches!(
            check(8, "device_trigger"),
            Err(Error::Unsupported {
                op: "device_trigger",
                ..
            })
        ));
        assert!(matches!(check(12, "x"), Err(Error::InvalidArgument(_))));
        let e = check(4, "device_read").unwrap_err();
        assert_eq!(
            e.to_string(),
            "vxi11: protocol error: device_read: device error 4 (invalid link identifier)"
        );
        assert!(check(99, "x").unwrap_err().to_string().contains("unknown"));
    }

    #[test]
    fn timeouts_in_milliseconds() {
        assert_eq!(millis(None), u32::MAX);
        assert_eq!(millis(Some(Duration::from_millis(2500))), 2500);
        assert_eq!(millis(Some(Duration::from_secs(u64::MAX))), u32::MAX - 1);
    }
}
