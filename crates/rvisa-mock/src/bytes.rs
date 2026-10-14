//! Readable, lossless byte strings for traces and logs.
//!
//! Printable ASCII is kept as is; every other byte, and `\`, becomes `\xNN`.

use std::fmt::Write as _;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Escapes `data` as described in the module docs.
#[must_use]
pub fn escape(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len());
    for &b in data {
        if b.is_ascii_graphic() && b != b'\\' || b == b' ' {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "\\x{b:02x}");
        }
    }
    out
}

/// Reverses [`escape`]. Returns `None` for malformed input.
#[must_use]
pub fn unescape(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'\\' {
            if bytes.get(i + 1) != Some(&b'x') {
                return None;
            }
            let hex = text.get(i + 2..i + 4)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 4;
        } else if b.is_ascii_graphic() || b == b' ' {
            out.push(b);
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

/// A byte string serialised with [`escape`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Bytes(pub Vec<u8>);

impl Bytes {
    /// Whether there are no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&escape(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        unescape(&text)
            .map(Bytes)
            .ok_or_else(|| serde::de::Error::custom("malformed escaped byte string"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_every_byte() {
        let all: Vec<u8> = (0..=255).collect();
        let text = escape(&all);
        assert!(text.is_ascii());
        assert_eq!(unescape(&text).unwrap(), all);
        assert_eq!(escape(b"*IDN?\n"), "*IDN?\\x0a");
        assert_eq!(escape(b"a\\b c"), "a\\x5cb c");
    }

    #[test]
    fn malformed() {
        for bad in ["\\", "\\x", "\\x0", "\\xzz", "\\y00", "tab\t", "é"] {
            assert_eq!(unescape(bad), None, "{bad:?}");
        }
    }
}
