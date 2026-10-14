//! Read framing for a byte stream without END (DESIGN.md S2).

use rvisa_core::{ReadResult, TermReason};

/// Bytes received from the instrument but not yet read.
///
/// A raw socket has no end-of-message indicator, so a read ends only at the
/// term char (if enabled) or after the requested count. [`take`](Self::take)
/// applies those rules; the blocking and async sessions differ only in how
/// they wait for more bytes.
///
/// ```
/// use rvisa_core::TermReason;
/// use rvisa_socket::ReadBuffer;
///
/// let mut buf = ReadBuffer::new();
/// buf.extend(b"1.5\n2.5");
/// let line = buf.take(64, Some(b'\n')).unwrap();
/// assert_eq!((line.data.as_slice(), line.reason), (&b"1.5\n"[..], TermReason::TermChar));
/// assert!(buf.take(64, Some(b'\n')).is_none(), "the rest has no term char yet");
/// assert_eq!(buf.take_partial(64), b"2.5");
/// ```
#[derive(Debug, Default, Clone)]
pub struct ReadBuffer {
    data: Vec<u8>,
    /// Bytes before this have been read. Advancing it instead of shifting
    /// the rest keeps many short reads (binary data full of term chars)
    /// linear in the data size.
    start: usize,
}

impl PartialEq for ReadBuffer {
    fn eq(&self, other: &Self) -> bool {
        self.pending() == other.pending()
    }
}

impl Eq for ReadBuffer {}

impl ReadBuffer {
    /// An empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn pending(&self) -> &[u8] {
        &self.data[self.start..]
    }

    /// The number of buffered bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len() - self.start
    }

    /// Whether nothing is buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Appends received bytes.
    pub fn extend(&mut self, bytes: &[u8]) {
        // Drop what has been read before growing, once it is most of the
        // buffer, so the copy is paid for by the reads that consumed it.
        if self.start > 0 && self.start >= self.data.len() / 2 {
            self.data.drain(..self.start);
            self.start = 0;
        }
        self.data.extend_from_slice(bytes);
    }

    fn consume(&mut self, n: usize) -> Vec<u8> {
        let taken = self.data[self.start..self.start + n].to_vec();
        self.start += n;
        if self.start == self.data.len() {
            self.data.clear();
            self.start = 0;
        }
        taken
    }

    /// Frames one read of at most `max` bytes: up to and including the
    /// first `term` among them ([`TermReason::TermChar`]), or else exactly
    /// `max` bytes ([`TermReason::MaxCount`]). Returns `None`, taking
    /// nothing, if neither is buffered yet. A `max` of zero takes nothing.
    pub fn take(&mut self, max: usize, term: Option<u8>) -> Option<ReadResult> {
        if max == 0 {
            return None;
        }
        let pending = self.pending();
        let window = &pending[..pending.len().min(max)];
        let (n, reason) = match term.and_then(|t| window.iter().position(|&b| b == t)) {
            Some(pos) => (pos + 1, TermReason::TermChar),
            None if pending.len() >= max => (max, TermReason::MaxCount),
            None => return None,
        };
        Some(ReadResult {
            data: self.consume(n),
            reason,
        })
    }

    /// Takes up to `max` bytes regardless of framing: what a read that
    /// timed out hands back (DESIGN.md C12).
    pub fn take_partial(&mut self, max: usize) -> Vec<u8> {
        self.consume(self.len().min(max))
    }

    /// Discards everything buffered.
    pub fn clear(&mut self) {
        self.data.clear();
        self.start = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(bytes: &[u8]) -> ReadBuffer {
        let mut b = ReadBuffer::new();
        b.extend(bytes);
        b
    }

    #[test]
    fn term_char_wins_within_max() {
        let mut b = buf(b"ab\ncd\n");
        let r = b.take(10, Some(b'\n')).unwrap();
        assert_eq!((r.data, r.reason), (b"ab\n".to_vec(), TermReason::TermChar));
        // The term char as the last byte that fits still counts as TermChar.
        let r = b.take(3, Some(b'\n')).unwrap();
        assert_eq!((r.data, r.reason), (b"cd\n".to_vec(), TermReason::TermChar));
        assert!(b.is_empty());
    }

    #[test]
    fn max_count_without_term() {
        let mut b = buf(b"abcdef\n");
        let r = b.take(4, Some(b'\n')).unwrap();
        assert_eq!((r.data, r.reason), (b"abcd".to_vec(), TermReason::MaxCount));
        // Without a term char only the count ends a read.
        assert!(b.take(4, None).is_none(), "3 of 4 bytes");
        let r = b.take(3, None).unwrap();
        assert_eq!((r.data, r.reason), (b"ef\n".to_vec(), TermReason::MaxCount));
    }

    #[test]
    fn incomplete_reads_wait() {
        let mut b = buf(b"abc");
        assert!(b.take(4, None).is_none());
        assert!(b.take(4, Some(b'\n')).is_none());
        assert!(b.take(0, None).is_none());
        assert_eq!(b.len(), 3, "nothing was taken");
        assert_eq!(b.take_partial(2), b"ab");
        assert_eq!(b.take_partial(10), b"c");
        assert!(b.take_partial(10).is_empty());
    }

    #[test]
    fn term_char_after_max_is_not_seen() {
        let mut b = buf(b"abcd\n");
        let r = b.take(2, Some(b'\n')).unwrap();
        assert_eq!(r.reason, TermReason::MaxCount);
        b.clear();
        assert!(b.is_empty());
    }

    #[test]
    fn many_short_reads_across_refills() {
        // Lines arrive in chunks that split them; every byte comes out once,
        // in order, however reads and refills interleave.
        let lines = if cfg!(miri) { 500 } else { 20_000u32 };
        let data: Vec<u8> = (0..lines)
            .flat_map(|i| format!("{i}\n").into_bytes())
            .collect();
        let mut b = ReadBuffer::new();
        let mut out = Vec::new();
        for chunk in data.chunks(777) {
            b.extend(chunk);
            while let Some(r) = b.take(5, Some(b'\n')) {
                out.extend(r.data);
            }
        }
        out.extend(b.take_partial(usize::MAX));
        assert_eq!(out, data);
        assert!(b.is_empty());
        assert_eq!(b, ReadBuffer::new());
    }
}
