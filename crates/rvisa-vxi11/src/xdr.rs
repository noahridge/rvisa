//! XDR encoding (RFC 4506), just the parts VXI-11 uses (DESIGN.md R2):
//! 32-bit integers, booleans, and variable-length opaque data and strings.

use rvisa_core::{Error, Result};

use crate::BACKEND;

/// Builds an XDR byte string.
#[derive(Debug, Default, Clone)]
pub(crate) struct Encoder {
    buf: Vec<u8>,
}

impl Encoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u32(u32::from(v))
    }

    /// Variable-length opaque data: a length, the bytes, and zero padding
    /// to a multiple of four.
    ///
    /// Data longer than `u32::MAX` bytes cannot be encoded; callers split
    /// writes far below that (`maxRecvSize`).
    pub fn opaque(&mut self, data: &[u8]) -> &mut Self {
        let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
        self.u32(len);
        self.buf.extend_from_slice(&data[..len as usize]);
        self.buf.resize(self.buf.len() + pad(len as usize), 0);
        self
    }

    pub fn string(&mut self, s: &str) -> &mut Self {
        self.opaque(s.as_bytes())
    }

    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }
}

/// Zero bytes after `len` bytes of opaque data.
fn pad(len: usize) -> usize {
    (4 - len % 4) % 4
}

/// Reads an XDR byte string. Every method fails cleanly on short or
/// malformed input; nothing panics (the decoder is fuzzed).
#[derive(Debug, Clone)]
pub(crate) struct Decoder<'a> {
    data: &'a [u8],
}

impl<'a> Decoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8]> {
        if self.data.len() < n {
            return Err(malformed(format!(
                "{what}: needs {n} bytes, {} left",
                self.data.len()
            )));
        }
        let (head, tail) = self.data.split_at(n);
        self.data = tail;
        Ok(head)
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4, "integer")?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        let b = self.take(4, "integer")?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn bool(&mut self) -> Result<bool> {
        match self.u32()? {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(malformed(format!("boolean {v}"))),
        }
    }

    /// Variable-length opaque data of at most `max` bytes.
    pub fn opaque(&mut self, max: usize) -> Result<&'a [u8]> {
        let len = self.u32()? as usize;
        if len > max {
            return Err(malformed(format!("opaque of {len} bytes, limit {max}")));
        }
        let data = self.take(len, "opaque data")?;
        self.take(pad(len), "opaque padding")?;
        Ok(data)
    }

    pub fn string(&mut self, max: usize) -> Result<String> {
        Ok(String::from_utf8_lossy(self.opaque(max)?).into_owned())
    }

    /// Skips opaque authentication data (RFC 5531 `opaque_auth` body).
    pub fn skip_opaque(&mut self, max: usize) -> Result<()> {
        self.opaque(max).map(|_| ())
    }

    /// The bytes not yet read.
    pub fn rest(&self) -> &'a [u8] {
        self.data
    }

    /// Fails unless everything was read.
    pub fn finish(&self) -> Result<()> {
        if self.data.is_empty() {
            Ok(())
        } else {
            Err(malformed(format!("{} trailing bytes", self.data.len())))
        }
    }
}

pub(crate) fn malformed(detail: impl std::fmt::Display) -> Error {
    Error::Protocol {
        backend: BACKEND,
        detail: format!("malformed XDR: {detail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let bytes = Encoder::new()
            .u32(7)
            .i32(-2)
            .bool(true)
            .opaque(b"abcde")
            .string("inst0")
            .opaque(b"")
            .finish();
        assert_eq!(bytes.len(), 4 * 3 + (4 + 8) + (4 + 8) + 4);
        assert_eq!(&bytes[..4], &[0, 0, 0, 7]);
        assert_eq!(&bytes[12..24], b"\0\0\0\x05abcde\0\0\0");

        let mut d = Decoder::new(&bytes);
        assert_eq!(d.u32().unwrap(), 7);
        assert_eq!(d.i32().unwrap(), -2);
        assert!(d.bool().unwrap());
        assert_eq!(d.opaque(16).unwrap(), b"abcde");
        assert_eq!(d.string(16).unwrap(), "inst0");
        assert_eq!(d.opaque(0).unwrap(), b"");
        d.finish().unwrap();
    }

    #[test]
    fn malformed_input_is_an_error() {
        fn protocol<T>(r: Result<T>) -> bool {
            matches!(r, Err(Error::Protocol { .. }))
        }
        assert!(protocol(Decoder::new(&[0, 0, 1]).u32()));
        assert!(protocol(Decoder::new(&[0, 0, 0, 2]).bool()));
        // Length beyond the limit, and beyond the data.
        assert!(protocol(Decoder::new(&[0, 0, 0, 9, 1, 2]).opaque(8)));
        assert!(protocol(Decoder::new(&[0, 0, 0, 3, 1, 2]).opaque(8)));
        // Missing padding.
        assert!(protocol(Decoder::new(&[0, 0, 0, 3, 1, 2, 3]).opaque(8)));
        assert!(protocol(
            Decoder::new(&[0xFF, 0xFF, 0xFF, 0xFF]).opaque(usize::MAX)
        ));
        assert!(protocol(Decoder::new(&[1]).finish()));
    }
}
