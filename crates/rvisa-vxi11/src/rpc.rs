//! A minimal ONC-RPC (RFC 5531) client over TCP, plus the message codecs
//! the test server and the SRQ interrupt server share (DESIGN.md R2).
//!
//! Only `AUTH_NONE` is used, as every VXI-11 implementation does. Records
//! use RFC 5531 record marking: each fragment starts with a 4-byte header,
//! a last-fragment bit and a 31-bit length.

// The server-side codecs serve the `server` and `srq` features.
#![cfg_attr(not(all(feature = "server", feature = "srq")), allow(dead_code))]

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use rvisa_core::{Error, Result};

use crate::BACKEND;
use crate::xdr::{Decoder, Encoder, malformed};

const CALL: u32 = 0;
const REPLY: u32 = 1;
const RPC_VERSION: u32 = 2;
const AUTH_NONE: u32 = 0;
const MSG_ACCEPTED: u32 = 0;
const MSG_DENIED: u32 = 1;
/// The most an `opaque_auth` body may hold (RFC 5531).
const MAX_AUTH: usize = 400;

const LAST_FRAGMENT: u32 = 0x8000_0000;

/// `accept_stat` values (RFC 5531).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AcceptStat {
    Success = 0,
    ProgUnavail = 1,
    ProgMismatch = 2,
    ProcUnavail = 3,
    GarbageArgs = 4,
}

/// How much one `recv` asks for.
const RECV_CHUNK: usize = 64 * 1024;

/// Takes one complete record (all its fragments) off the front of `buf`,
/// or returns `None` if it has not all arrived. Records longer than `max`
/// are an error.
pub(crate) fn take_record(buf: &mut Vec<u8>, max: usize) -> Result<Option<Vec<u8>>> {
    let mut pos = 0;
    let mut len = 0usize;
    loop {
        let Some(header) = buf.get(pos..pos + 4) else {
            return Ok(None);
        };
        let header = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let fragment = (header & !LAST_FRAGMENT) as usize;
        len = len.saturating_add(fragment);
        if len > max {
            return Err(malformed(format!("RPC record of {len} bytes, limit {max}")));
        }
        let end = pos + 4 + fragment;
        if buf.len() < end {
            return Ok(None);
        }
        pos = end;
        if header & LAST_FRAGMENT != 0 {
            break;
        }
    }
    let mut record = Vec::with_capacity(len);
    let mut at = 0;
    while at < pos {
        let fragment = (u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
            & !LAST_FRAGMENT) as usize;
        record.extend_from_slice(&buf[at + 4..at + 4 + fragment]);
        at += 4 + fragment;
    }
    buf.drain(..pos);
    Ok(Some(record))
}

/// Frames `body` as one record of one fragment.
pub(crate) fn frame(body: &[u8]) -> Vec<u8> {
    // Bodies are far below 2 GiB: writes are split by `maxRecvSize`.
    let len = u32::try_from(body.len()).unwrap_or(!LAST_FRAGMENT) & !LAST_FRAGMENT;
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(LAST_FRAGMENT | len).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// A call message with `AUTH_NONE`.
pub(crate) fn encode_call(xid: u32, prog: u32, vers: u32, proc: u32, args: &[u8]) -> Vec<u8> {
    let mut e = Encoder::new();
    e.u32(xid)
        .u32(CALL)
        .u32(RPC_VERSION)
        .u32(prog)
        .u32(vers)
        .u32(proc)
        .u32(AUTH_NONE)
        .u32(0)
        .u32(AUTH_NONE)
        .u32(0);
    let mut out = e.finish();
    out.extend_from_slice(args);
    out
}

/// Decodes a reply. Returns its xid and, for a successful call, the
/// results.
pub(crate) fn decode_reply(body: &[u8]) -> Result<(u32, Result<&[u8]>)> {
    let mut d = Decoder::new(body);
    let xid = d.u32()?;
    if d.u32()? != REPLY {
        return Err(malformed("expected an RPC reply"));
    }
    let result = match d.u32()? {
        MSG_ACCEPTED => {
            d.u32()?; // verifier flavour
            d.skip_opaque(MAX_AUTH)?;
            match d.u32()? {
                0 => Ok(d.rest()),
                2 => {
                    let (low, high) = (d.u32()?, d.u32()?);
                    Err(rpc_error(format!(
                        "program version unavailable (server has {low}–{high})"
                    )))
                }
                stat => Err(rpc_error(match stat {
                    1 => "program unavailable".to_owned(),
                    3 => "procedure unavailable".to_owned(),
                    4 => "the server could not decode the arguments".to_owned(),
                    5 => "server system error".to_owned(),
                    _ => format!("accept status {stat}"),
                })),
            }
        }
        MSG_DENIED => Err(rpc_error(match d.u32()? {
            0 => "RPC version mismatch".to_owned(),
            1 => format!("authentication error {}", d.u32()?),
            stat => format!("rejected with status {stat}"),
        })),
        stat => return Err(malformed(format!("reply status {stat}"))),
    };
    Ok((xid, result))
}

fn rpc_error(detail: String) -> Error {
    Error::Protocol {
        backend: BACKEND,
        detail: format!("RPC: {detail}"),
    }
}

/// A decoded call message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Call<'a> {
    pub xid: u32,
    pub prog: u32,
    pub vers: u32,
    pub proc: u32,
    pub args: &'a [u8],
}

pub(crate) fn decode_call(body: &[u8]) -> Result<Call<'_>> {
    let mut d = Decoder::new(body);
    let xid = d.u32()?;
    if d.u32()? != CALL {
        return Err(malformed("expected an RPC call"));
    }
    if d.u32()? != RPC_VERSION {
        return Err(malformed("RPC version is not 2"));
    }
    let (prog, vers, proc) = (d.u32()?, d.u32()?, d.u32()?);
    for _ in 0..2 {
        d.u32()?; // credential, then verifier, flavour
        d.skip_opaque(MAX_AUTH)?;
    }
    Ok(Call {
        xid,
        prog,
        vers,
        proc,
        args: d.rest(),
    })
}

/// An accepted reply with `AUTH_NONE`: the results for `Success`, the
/// supported version range for `ProgMismatch`, nothing otherwise.
pub(crate) fn encode_reply(xid: u32, stat: AcceptStat, results: &[u8]) -> Vec<u8> {
    let mut e = Encoder::new();
    e.u32(xid)
        .u32(REPLY)
        .u32(MSG_ACCEPTED)
        .u32(AUTH_NONE)
        .u32(0)
        .u32(stat as u32);
    let mut out = e.finish();
    out.extend_from_slice(results);
    out
}

/// Writes all of `data`, retrying on interrupts.
pub(crate) fn send(stream: &mut TcpStream, data: &[u8]) -> Result<()> {
    stream.write_all(data).map_err(|e| match e.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => Error::timeout(),
        _ => e.into(),
    })
}

/// An RPC client on one TCP connection.
///
/// A call that times out leaves its reply in flight; the next call skips
/// replies whose xid is not its own.
#[derive(Debug)]
pub(crate) struct RpcClient {
    stream: TcpStream,
    rx: Vec<u8>,
    scratch: Box<[u8]>,
    next_xid: u32,
    max_record: usize,
}

impl RpcClient {
    pub fn new(stream: TcpStream, max_record: usize) -> Self {
        Self {
            stream,
            rx: Vec::new(),
            scratch: vec![0; RECV_CHUNK].into_boxed_slice(),
            // Distinct starting xids per connection make stray replies
            // from a server bug easier to spot in a capture.
            next_xid: std::process::id().rotate_left(16) ^ 0x5658_0000,
            max_record,
        }
    }

    pub fn connect(addr: SocketAddr, timeout: Duration, max_record: usize) -> Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, timeout.max(Duration::from_millis(1)))
            .map_err(|e| match e.kind() {
                ErrorKind::TimedOut | ErrorKind::WouldBlock => Error::timeout(),
                _ => e.into(),
            })?;
        stream.set_nodelay(true)?;
        Ok(Self::new(stream, max_record))
    }

    pub fn stream(&self) -> &TcpStream {
        &self.stream
    }

    /// Calls `proc` and waits up to `wait` (`None`: forever) for the
    /// reply. Returns the results.
    pub fn call(
        &mut self,
        prog: u32,
        vers: u32,
        proc: u32,
        args: &[u8],
        wait: Option<Duration>,
    ) -> Result<Vec<u8>> {
        let xid = self.next_xid;
        self.next_xid = self.next_xid.wrapping_add(1);
        let deadline = wait.map(|w| Instant::now() + w);
        self.stream
            .set_write_timeout(wait.map(|w| w.max(Duration::from_millis(1))))?;
        send(
            &mut self.stream,
            &frame(&encode_call(xid, prog, vers, proc, args)),
        )?;
        loop {
            while let Some(record) = take_record(&mut self.rx, self.max_record)? {
                let (got, result) = decode_reply(&record)?;
                if got == xid {
                    return result.map(<[u8]>::to_vec);
                }
                tracing::debug!(xid = got, "vxi11: discarding a stale RPC reply");
            }
            self.fill(deadline)?;
        }
    }

    /// Receives once, waiting until `deadline` at most.
    fn fill(&mut self, deadline: Option<Instant>) -> Result<()> {
        let wait = match deadline {
            None => None,
            Some(d) => match d.checked_duration_since(Instant::now()) {
                Some(left) if !left.is_zero() => Some(left.max(Duration::from_millis(1))),
                _ => return Err(Error::timeout()),
            },
        };
        self.stream.set_read_timeout(wait)?;
        match self.stream.read(&mut self.scratch) {
            Ok(0) => Err(closed()),
            Ok(n) => {
                self.rx.extend_from_slice(&self.scratch[..n]);
                Ok(())
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => Ok(()),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                Err(Error::timeout())
            }
            Err(e) => Err(e.into()),
        }
    }
}

pub(crate) fn closed() -> Error {
    std::io::Error::new(
        ErrorKind::UnexpectedEof,
        "the instrument closed the connection",
    )
    .into()
}

/// Reads records from a stream for a server: buffers input and yields
/// complete records.
#[derive(Debug, Default)]
pub(crate) struct RecordReader {
    rx: Vec<u8>,
}

impl RecordReader {
    /// Receives once (honouring the stream's read timeout) and returns the
    /// next complete record, if any. `Ok(None)` also for a timeout.
    pub fn next(&mut self, stream: &mut TcpStream, max: usize) -> std::io::Result<Option<Vec<u8>>> {
        let to_io = |e: Error| std::io::Error::new(ErrorKind::InvalidData, e.to_string());
        if let Some(record) = take_record(&mut self.rx, max).map_err(to_io)? {
            return Ok(Some(record));
        }
        let mut buf = [0u8; 16 * 1024];
        match stream.read(&mut buf) {
            Ok(0) => Err(ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                self.rx.extend_from_slice(&buf[..n]);
                take_record(&mut self.rx, max).map_err(to_io)
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_reassemble_fragments() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&3u32.to_be_bytes());
        buf.extend_from_slice(b"abc");
        assert_eq!(take_record(&mut buf, 100).unwrap(), None, "not the last");
        buf.extend_from_slice(&(LAST_FRAGMENT | 2).to_be_bytes());
        buf.extend_from_slice(b"d");
        assert_eq!(take_record(&mut buf, 100).unwrap(), None, "incomplete");
        buf.extend_from_slice(b"e");
        buf.extend_from_slice(&frame(b"next"));
        assert_eq!(take_record(&mut buf, 100).unwrap().unwrap(), b"abcde");
        assert_eq!(take_record(&mut buf, 100).unwrap().unwrap(), b"next");
        assert!(buf.is_empty());

        let mut big = frame(&[0; 10]);
        assert!(take_record(&mut big, 9).is_err());
        let mut huge = (LAST_FRAGMENT | 0x7FFF_FFFF).to_be_bytes().to_vec();
        assert!(take_record(&mut huge, 1 << 20).is_err());
    }

    #[test]
    fn calls_and_replies_roundtrip() {
        let call = encode_call(42, 0x0607AF, 1, 10, b"ARGS");
        assert_eq!(
            decode_call(&call).unwrap(),
            Call {
                xid: 42,
                prog: 0x0607AF,
                vers: 1,
                proc: 10,
                args: b"ARGS"
            }
        );
        let reply = encode_reply(42, AcceptStat::Success, b"RES");
        let (xid, results) = decode_reply(&reply).unwrap();
        assert_eq!((xid, results.unwrap()), (42, &b"RES"[..]));

        let reply = encode_reply(7, AcceptStat::ProcUnavail, b"");
        let (xid, results) = decode_reply(&reply).unwrap();
        assert_eq!(xid, 7);
        assert!(matches!(results, Err(Error::Protocol { .. })));

        let mismatch = encode_reply(1, AcceptStat::ProgMismatch, &[0, 0, 0, 1, 0, 0, 0, 1]);
        let err = decode_reply(&mismatch).unwrap().1.unwrap_err();
        assert!(err.to_string().contains("1–1"), "{err}");

        assert!(decode_reply(&call).is_err(), "a call is not a reply");
        assert!(decode_call(&reply).is_err(), "a reply is not a call");
        assert!(decode_reply(&[0, 0, 0, 1]).is_err());
    }
}
