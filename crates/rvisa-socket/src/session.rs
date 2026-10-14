//! The blocking socket session (DESIGN.md S2–S6).

use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rvisa_core::{
    Attr, AttrValue, Error, Event, EventKind, LockKind, ReadResult, ResourceName, Result, Session,
    TermReason,
};
use socket2::SockRef;

use crate::backend::{LOCK_POLL, LockRegistry};
use crate::buffer::ReadBuffer;
use crate::settings::{Options, STB_REPLY_MAX, Settings, parse_stb, socket_timeout, unsupported};

/// How much one `recv` asks for.
const RECV_CHUNK: usize = 64 * 1024;

/// A blocking session on a raw TCP socket. Open it with
/// [`SocketBackend`](crate::SocketBackend).
#[derive(Debug)]
pub struct SocketSession {
    stream: TcpStream,
    resource: ResourceName,
    buf: ReadBuffer,
    scratch: Box<[u8]>,
    settings: Settings,
    locks: Arc<LockRegistry>,
    id: u64,
}

impl SocketSession {
    pub(crate) fn new(
        stream: TcpStream,
        resource: ResourceName,
        options: &Options,
        locks: Arc<LockRegistry>,
    ) -> Result<Self> {
        stream.set_nodelay(options.nodelay)?;
        SockRef::from(&stream).set_keepalive(options.keepalive)?;
        let id = locks.new_id();
        Ok(Self {
            stream,
            resource,
            buf: ReadBuffer::new(),
            scratch: vec![0; RECV_CHUNK].into_boxed_slice(),
            settings: Settings::new(options),
            locks,
            id,
        })
    }

    /// The connected peer's address.
    ///
    /// # Errors
    ///
    /// I/O errors from the socket.
    pub fn peer_addr(&self) -> Result<std::net::SocketAddr> {
        Ok(self.stream.peer_addr()?)
    }

    fn check_access(&self) -> Result<()> {
        self.locks.check(&self.resource, self.id)
    }

    /// Receives once into the buffer, waiting until `deadline` at most.
    /// Returns without error if the wait ended without data.
    fn fill(&mut self, deadline: Option<Instant>) -> Result<()> {
        let wait = match deadline {
            None => None,
            Some(d) => match d.checked_duration_since(Instant::now()) {
                Some(left) if !left.is_zero() => Some(socket_timeout(left)),
                _ => return Ok(()),
            },
        };
        self.stream.set_read_timeout(wait)?;
        match self.stream.read(&mut self.scratch) {
            Ok(0) => Err(closed()),
            Ok(n) => {
                tracing::trace!(resource = %self.resource, len = n, data = ?String::from_utf8_lossy(&self.scratch[..n]), "socket: recv");
                self.buf.extend(&self.scratch[..n]);
                Ok(())
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// One framed read of at most `max` bytes (DESIGN.md S2).
    fn read_framed(&mut self, max: usize, term: Option<u8>) -> Result<ReadResult> {
        let deadline = self.settings.timeout.map(|t| Instant::now() + t);
        loop {
            if let Some(read) = self.buf.take(max, term) {
                return Ok(read);
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                let partial = self.buf.take_partial(max);
                tracing::trace!(resource = %self.resource, len = partial.len(), "socket: read timed out");
                return Err(Error::read_timeout(partial));
            }
            self.fill(deadline)?;
        }
    }

    /// Discards received input: the buffer, and whatever the socket has
    /// already received (DESIGN.md S4).
    fn drain(&mut self) -> Result<()> {
        self.buf.clear();
        self.stream.set_nonblocking(true)?;
        let result = loop {
            match self.stream.read(&mut self.scratch) {
                Ok(0) => break Err(closed()),
                Ok(n) => tracing::trace!(resource = %self.resource, len = n, "socket: drained"),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => break Ok(()),
                Err(e) => break Err(e.into()),
            }
        };
        self.stream.set_nonblocking(false)?;
        result
    }

    fn send_all(&mut self, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            let n = self.write(data)?;
            data = &data[n..];
        }
        Ok(())
    }
}

fn closed() -> Error {
    std::io::Error::new(
        ErrorKind::UnexpectedEof,
        "the instrument closed the connection",
    )
    .into()
}

impl Drop for SocketSession {
    fn drop(&mut self) {
        self.locks.release_all(&self.resource, self.id);
    }
}

impl Session for SocketSession {
    fn resource(&self) -> &ResourceName {
        &self.resource
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.check_access()?;
        if data.is_empty() {
            return Ok(0);
        }
        self.stream
            .set_write_timeout(self.settings.timeout.map(socket_timeout))?;
        tracing::trace!(resource = %self.resource, len = data.len(), data = ?String::from_utf8_lossy(data), "socket: write");
        let mut sent = 0;
        while sent < data.len() {
            match self.stream.write(&data[sent..]) {
                Ok(0) => return Err(closed()),
                Ok(n) => sent += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                // A partial write is reported as such; `write_all` retries.
                Err(_) if sent > 0 => break,
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return Err(Error::timeout());
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(sent)
    }

    fn read(&mut self, max: usize) -> Result<ReadResult> {
        self.check_access()?;
        if max == 0 {
            return Err(Error::InvalidArgument("read of zero bytes".into()));
        }
        let read = self.read_framed(max, self.settings.term())?;
        tracing::trace!(resource = %self.resource, len = read.data.len(), reason = ?read.reason, "socket: read");
        Ok(read)
    }

    fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<()> {
        self.settings.timeout = timeout;
        Ok(())
    }

    fn timeout(&self) -> Option<Duration> {
        self.settings.timeout
    }

    fn set_read_termination(&mut self, term: Option<u8>) -> Result<()> {
        self.settings.set_read_termination(term);
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        self.check_access()?;
        self.drain()?;
        if let Some(command) = self.settings.clear_command.clone() {
            self.send_all(&command)?;
        }
        Ok(())
    }

    fn read_stb(&mut self) -> Result<u8> {
        if !self.settings.strings {
            return Err(unsupported("read_stb"));
        }
        self.check_access()?;
        self.send_all(b"*STB?\n")?;
        // The reply is read up to `\n` whatever the term char setting (S5).
        let reply = self.read_framed(STB_REPLY_MAX, Some(b'\n'))?;
        if reply.reason != TermReason::TermChar {
            return Err(Error::InvalidData(format!(
                "*STB? reply longer than {STB_REPLY_MAX} bytes"
            )));
        }
        parse_stb(&reply.data)
    }

    fn assert_trigger(&mut self) -> Result<()> {
        if !self.settings.strings {
            return Err(unsupported("assert_trigger"));
        }
        self.check_access()?;
        self.send_all(b"*TRG\n")
    }

    fn lock(&mut self, kind: LockKind, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.locks.try_lock(&self.resource, self.id, &kind) {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(Error::ResourceLocked);
            }
            std::thread::sleep(LOCK_POLL.min(deadline - now));
        }
    }

    fn unlock(&mut self) -> Result<()> {
        self.locks.unlock(&self.resource, self.id)
    }

    fn get_attr(&self, attr: Attr) -> Result<AttrValue> {
        if let Some(value) = self.settings.get(attr, &self.resource)? {
            return Ok(value);
        }
        Ok(AttrValue::Bool(match attr {
            Attr::TcpipNodelay => self.stream.nodelay()?,
            _ => SockRef::from(&self.stream).keepalive()?,
        }))
    }

    fn set_attr(&mut self, attr: Attr, value: AttrValue) -> Result<()> {
        if self.settings.set(attr, &value)? {
            return Ok(());
        }
        let on = value.as_bool().unwrap_or_default();
        match attr {
            Attr::TcpipNodelay => self.stream.set_nodelay(on)?,
            _ => SockRef::from(&self.stream).set_keepalive(on)?,
        }
        Ok(())
    }

    /// A raw socket has no events (DESIGN.md S6).
    fn enable_event(&mut self, _kind: EventKind) -> Result<()> {
        Err(unsupported("enable_event"))
    }

    fn wait_event(&mut self, _kind: EventKind, _timeout: Duration) -> Result<Event> {
        Err(unsupported("wait_event"))
    }

    fn disable_event(&mut self, _kind: EventKind) -> Result<()> {
        Err(unsupported("disable_event"))
    }
}
