//! IEEE 488.2 arbitrary block data: definite length (`#<n><length><data>`)
//! and indefinite length (`#0<data>` terminated by newline with END).
//!
//! The parsers never trust the length field for allocation, and reject
//! malformed headers with [`Error::InvalidData`] instead of panicking.

use crate::error::{Error, Result};

/// Where a block's data starts, and how long it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockHeader {
    /// `#<n><length>`: exactly `len` bytes follow.
    Definite {
        /// Offset of the first data byte in the buffer.
        data_start: usize,
        /// Number of data bytes.
        len: usize,
    },
    /// `#0`: data runs until the final newline, which is sent with END.
    Indefinite {
        /// Offset of the first data byte in the buffer.
        data_start: usize,
    },
}

fn invalid(reason: &str) -> Error {
    Error::InvalidData(format!("IEEE 488.2 block: {reason}"))
}

/// Parses the header at the start of a (possibly partial) response.
///
/// Bytes before the `#` are skipped, since some instruments echo a command
/// header first (`CURV #41000...`). Returns `Ok(None)` if more bytes are
/// needed to complete the header.
///
/// # Errors
///
/// [`Error::InvalidData`] if the header is malformed.
pub fn parse_header(buf: &[u8]) -> Result<Option<BlockHeader>> {
    let Some(hash) = buf.iter().position(|&b| b == b'#') else {
        return Ok(None);
    };
    let Some(&count) = buf.get(hash + 1) else {
        return Ok(None);
    };
    if !count.is_ascii_digit() {
        return Err(invalid("length-of-length is not a digit"));
    }
    let digits = usize::from(count - b'0');
    let length_start = hash + 2;
    if digits == 0 {
        return Ok(Some(BlockHeader::Indefinite {
            data_start: length_start,
        }));
    }
    let Some(length) = buf.get(length_start..length_start + digits) else {
        return Ok(None);
    };
    if !length.iter().all(u8::is_ascii_digit) {
        return Err(invalid("length is not a decimal number"));
    }
    // At most nine digits, so this cannot overflow a usize of 32 bits or more.
    let len = length
        .iter()
        .fold(0usize, |acc, &d| acc * 10 + usize::from(d - b'0'));
    Ok(Some(BlockHeader::Definite {
        data_start: length_start + digits,
        len,
    }))
}

/// Extracts the data of a block from a complete response message.
///
/// For a definite-length block, bytes after the data (the message
/// terminator) are ignored. For an indefinite-length block, a final `\n`
/// is removed.
///
/// # Errors
///
/// [`Error::InvalidData`] if there is no header, the header is malformed,
/// or the message is shorter than the declared length.
pub fn parse_block(message: &[u8]) -> Result<&[u8]> {
    match parse_header(message)? {
        None => Err(invalid("no complete '#' header")),
        Some(BlockHeader::Indefinite { data_start }) => {
            let data = &message[data_start..];
            Ok(data.strip_suffix(b"\n").unwrap_or(data))
        }
        Some(BlockHeader::Definite { data_start, len }) => message
            .get(data_start..)
            .and_then(|rest| rest.get(..len))
            .ok_or_else(|| {
                invalid(&format!(
                    "declared {len} bytes but only {} arrived",
                    message.len().saturating_sub(data_start)
                ))
            }),
    }
}

/// The definite-length header for `len` bytes of data, such as `#41000`.
///
/// # Errors
///
/// [`Error::InvalidArgument`] if `len` needs more than the nine digits a
/// header can hold (1 GB or more).
pub fn encode_header(len: usize) -> Result<String> {
    let digits = len.to_string();
    if digits.len() > 9 {
        return Err(Error::InvalidArgument(format!(
            "IEEE 488.2 block: {len} bytes is too long for a definite-length header"
        )));
    }
    Ok(format!("#{}{digits}", digits.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers() {
        assert_eq!(parse_header(b"").unwrap(), None);
        assert_eq!(parse_header(b"CURV ").unwrap(), None);
        assert_eq!(parse_header(b"#").unwrap(), None);
        assert_eq!(parse_header(b"#3").unwrap(), None);
        assert_eq!(parse_header(b"#31").unwrap(), None);
        assert_eq!(
            parse_header(b"#3100").unwrap(),
            Some(BlockHeader::Definite {
                data_start: 5,
                len: 100
            })
        );
        assert_eq!(
            parse_header(b"CURV #0abc").unwrap(),
            Some(BlockHeader::Indefinite { data_start: 7 })
        );
        assert_eq!(
            parse_header(b"#9999999999").unwrap(),
            Some(BlockHeader::Definite {
                data_start: 11,
                len: 999_999_999
            })
        );
        assert!(parse_header(b"#x").is_err());
        assert!(parse_header(b"#2a1").is_err());
    }

    #[test]
    fn blocks() {
        assert_eq!(parse_block(b"#15hello\n").unwrap(), b"hello");
        assert_eq!(parse_block(b"#15hello").unwrap(), b"hello");
        assert_eq!(parse_block(b"#10").unwrap(), b"");
        assert_eq!(parse_block(b"#0abc\n").unwrap(), b"abc");
        assert_eq!(parse_block(b"#0a\nb\n").unwrap(), b"a\nb");
        assert_eq!(parse_block(b"#0").unwrap(), b"");
        assert!(parse_block(b"hello").is_err());
        assert!(parse_block(b"#2").is_err());
        assert!(parse_block(b"#15abc").is_err());
        assert!(parse_block(b"#9999999999").is_err());
    }

    #[test]
    fn header_encoding_round_trips() {
        for len in [0, 1, 9, 10, 1000, 1_048_576] {
            let header = encode_header(len).unwrap();
            assert_eq!(
                parse_header(header.as_bytes()).unwrap(),
                Some(BlockHeader::Definite {
                    data_start: header.len(),
                    len
                })
            );
        }
    }
}
