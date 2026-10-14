//! A native async socket session on tokio (DESIGN.md S7).
//!
//! [`AsyncSocketSession`] behaves like the blocking
//! [`SocketSession`](crate::SocketSession): the same framing, options,
//! errors and locks. Because it is used directly rather than through the
//! `rvisa` facade, it also carries the facade's write termination (`\n` by
//! default) for [`query`](AsyncSocketSession::query) and
//! [`write_str`](AsyncSocketSession::write_str).
//!
//! ```no_run
//! # async fn demo() -> rvisa_core::Result<()> {
//! use rvisa_socket::SocketBackend;
//! use rvisa_socket::tokio::AsyncSocketSession;
//!
//! let backend = SocketBackend::new();
//! let mut dmm = AsyncSocketSession::connect(&backend, &"TCPIP0::192.168.1.50::5025::SOCKET".parse()?).await?;
//! dmm.set_read_termination(Some(b'\n'));
//! let idn = dmm.query("*IDN?").await?;
//! # Ok(()) }
//! ```

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rvisa_core::ieee488::{self, BlockHeader};
use rvisa_core::{
    Attr, AttrValue, Error, LockKind, READ_CHUNK, ReadResult, ResourceName, Result, TermReason,
};
use socket2::SockRef;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Instant;

use crate::SocketBackend;
use crate::backend::{LOCK_POLL, LockRegistry};
use crate::buffer::ReadBuffer;
use crate::settings::{STB_REPLY_MAX, Settings, parse_stb, unsupported};

const RECV_CHUNK: usize = 64 * 1024;

/// An async session on a raw TCP socket.
///
/// Dropping a read future loses no data: received bytes stay buffered in
/// the session until a read frames them.
#[derive(Debug)]
pub struct AsyncSocketSession {
    stream: TcpStream,
    resource: ResourceName,
    buf: ReadBuffer,
    scratch: Box<[u8]>,
    settings: Settings,
    write_termination: Option<Vec<u8>>,
    locks: Arc<LockRegistry>,
    id: u64,
}

impl AsyncSocketSession {
    /// Connects with `backend`'s options. Sessions from the same backend,
    /// blocking or async, share its locks.
    ///
    /// # Errors
    ///
    /// As [`SocketBackend::open_socket`].
    pub async fn connect(backend: &SocketBackend, resource: &ResourceName) -> Result<Self> {
        let ResourceName::TcpipSocket { host, port, .. } = resource else {
            return Err(Error::InvalidArgument(format!(
                "{resource} is not a SOCKET resource"
            )));
        };
        let options = &backend.options;
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), *port))
            .await
            .map_err(|_| Error::NotFound(resource.clone()))?
            .collect();
        let mut last = None;
        let mut stream = None;
        for addr in addrs {
            match tokio::time::timeout(options.connect_timeout, TcpStream::connect(addr)).await {
                Ok(Ok(s)) => {
                    stream = Some(s);
                    break;
                }
                Ok(Err(e)) => last = Some(Error::from(e)),
                Err(_) => last = Some(Error::timeout()),
            }
        }
        let stream = match (stream, last) {
            (Some(s), _) => s,
            (None, Some(Error::Io(e))) if e.kind() == ErrorKind::ConnectionRefused => {
                return Err(Error::NotFound(resource.clone()));
            }
            (None, Some(e)) => return Err(e),
            (None, None) => return Err(Error::NotFound(resource.clone())),
        };
        stream.set_nodelay(options.nodelay)?;
        SockRef::from(&stream).set_keepalive(options.keepalive)?;
        let locks = Arc::clone(&backend.locks);
        let id = locks.new_id();
        Ok(Self {
            stream,
            resource: resource.clone(),
            buf: ReadBuffer::new(),
            scratch: vec![0; RECV_CHUNK].into_boxed_slice(),
            settings: Settings::new(options),
            write_termination: Some(b"\n".to_vec()),
            locks,
            id,
        })
    }

    /// The resource this session is connected to.
    #[must_use]
    pub fn resource(&self) -> &ResourceName {
        &self.resource
    }

    /// The I/O timeout; `None` is infinite.
    #[must_use]
    pub fn timeout(&self) -> Option<Duration> {
        self.settings.timeout
    }

    /// Sets the I/O timeout; `None` is infinite.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.settings.timeout = timeout;
    }

    /// Sets the read termination character, or disables it with `None`.
    pub fn set_read_termination(&mut self, term: Option<u8>) {
        self.settings.set_read_termination(term);
    }

    /// The bytes [`write_str`](Self::write_str) and [`query`](Self::query)
    /// append (`\n` by default).
    #[must_use]
    pub fn write_termination(&self) -> Option<&[u8]> {
        self.write_termination.as_deref()
    }

    /// Sets the write termination, or disables it with `None`.
    pub fn set_write_termination(&mut self, term: Option<&[u8]>) {
        self.write_termination = term.map(<[u8]>::to_vec);
    }

    /// Reads an attribute; the same set as the blocking session.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] for attributes a socket does not have.
    pub fn get_attr(&self, attr: Attr) -> Result<AttrValue> {
        if let Some(value) = self.settings.get(attr, &self.resource)? {
            return Ok(value);
        }
        Ok(AttrValue::Bool(match attr {
            Attr::TcpipNodelay => self.stream.nodelay()?,
            _ => SockRef::from(&self.stream).keepalive()?,
        }))
    }

    /// Writes an attribute; the same set as the blocking session.
    ///
    /// # Errors
    ///
    /// [`Error::AttrReadOnly`], [`Error::InvalidArgument`] for a value of
    /// the wrong type, or [`Error::Unsupported`].
    pub fn set_attr(&mut self, attr: Attr, value: AttrValue) -> Result<()> {
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

    fn check_access(&self) -> Result<()> {
        self.locks.check(&self.resource, self.id)
    }

    fn deadline(&self) -> Option<Instant> {
        self.settings.timeout.map(|t| Instant::now() + t)
    }

    /// Writes all of `data` unchanged. Returns its length.
    ///
    /// # Errors
    ///
    /// [`Error::Timeout`], [`Error::ResourceLocked`], or I/O errors.
    pub async fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.check_access()?;
        tracing::trace!(resource = %self.resource, len = data.len(), data = ?String::from_utf8_lossy(data), "socket async: write");
        let write = self.stream.write_all(data);
        match self.settings.timeout {
            Some(t) => tokio::time::timeout(t, write)
                .await
                .map_err(|_| Error::timeout())??,
            None => write.await?,
        }
        Ok(data.len())
    }

    /// Writes `command` and the write termination.
    ///
    /// # Errors
    ///
    /// As [`write`](Self::write).
    pub async fn write_str(&mut self, command: &str) -> Result<()> {
        let mut message = command.as_bytes().to_vec();
        if let Some(term) = &self.write_termination {
            message.extend_from_slice(term);
        }
        self.write(&message).await.map(drop)
    }

    async fn read_framed(&mut self, max: usize, term: Option<u8>) -> Result<ReadResult> {
        let deadline = self.deadline();
        loop {
            if let Some(read) = self.buf.take(max, term) {
                return Ok(read);
            }
            let recv = self.stream.read(&mut self.scratch);
            let received = match deadline {
                Some(d) => match tokio::time::timeout_at(d, recv).await {
                    Ok(r) => r,
                    Err(_) => return Err(Error::read_timeout(self.buf.take_partial(max))),
                },
                None => recv.await,
            };
            match received {
                Ok(0) => return Err(closed()),
                Ok(n) => self.buf.extend(&self.scratch[..n]),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Reads up to `max` bytes, stopping at the term char if enabled;
    /// otherwise only after `max` bytes (DESIGN.md S2).
    ///
    /// # Errors
    ///
    /// [`Error::Timeout`] carrying any bytes received, or I/O errors.
    pub async fn read(&mut self, max: usize) -> Result<ReadResult> {
        self.check_access()?;
        if max == 0 {
            return Err(Error::InvalidArgument("read of zero bytes".into()));
        }
        self.read_framed(max, self.settings.term()).await
    }

    /// Reads one message: until the term char. Without the term char
    /// enabled this only ends at a timeout.
    ///
    /// # Errors
    ///
    /// As [`read`](Self::read); a timeout carries the whole message so far.
    pub async fn read_message(&mut self) -> Result<Vec<u8>> {
        let mut message = Vec::new();
        loop {
            let chunk = match self.read(READ_CHUNK).await {
                Ok(c) => c,
                Err(e) => return Err(e.prepend_partial(message)),
            };
            message.extend_from_slice(&chunk.data);
            if chunk.reason.is_message_end() {
                return Ok(message);
            }
        }
    }

    /// Writes `command` with the write termination and returns the reply
    /// as text, without trailing `\r`/`\n`.
    ///
    /// # Errors
    ///
    /// Errors from writing or reading.
    pub async fn query(&mut self, command: &str) -> Result<String> {
        self.write_str(command).await?;
        let reply = self.read_message().await?;
        Ok(String::from_utf8_lossy(&reply)
            .trim_end_matches(['\r', '\n'])
            .to_owned())
    }

    /// Reads a definite-length IEEE 488.2 block and the terminator after
    /// it, returning the data. Indefinite-length blocks need END, which a
    /// socket does not have.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidData`] for a malformed or indefinite block, and
    /// errors from reading.
    pub async fn read_ieee488_block(&mut self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        let (data_start, len) = loop {
            match ieee488::parse_header(&buf)? {
                Some(BlockHeader::Definite { data_start, len }) => break (data_start, len),
                Some(BlockHeader::Indefinite { .. }) => {
                    return Err(Error::InvalidData(
                        "indefinite-length blocks need END, which a socket lacks".into(),
                    ));
                }
                None => {}
            }
            match self.read_framed(1, None).await {
                Ok(byte) => buf.extend_from_slice(&byte.data),
                Err(e) => return Err(e.prepend_partial(buf)),
            }
        };
        let end = data_start.saturating_add(len);
        while buf.len() < end {
            let want = (end - buf.len()).min(READ_CHUNK);
            match self.read_framed(want, None).await {
                Ok(chunk) => buf.extend_from_slice(&chunk.data),
                Err(e) => return Err(e.prepend_partial(buf)),
            }
        }
        // The message terminator follows the data.
        if let Some(term) = self.settings.term() {
            match self.read_framed(READ_CHUNK, Some(term)).await {
                Ok(rest) if rest.reason == TermReason::TermChar => {}
                Ok(_) => {
                    return Err(Error::InvalidData(
                        "IEEE 488.2 block: no terminator after the data".into(),
                    ));
                }
                Err(e) => return Err(e.prepend_partial(buf)),
            }
        }
        buf.truncate(end);
        buf.drain(..data_start);
        Ok(buf)
    }

    /// Writes `command` with the write termination and reads a
    /// definite-length IEEE 488.2 block.
    ///
    /// # Errors
    ///
    /// As [`write_str`](Self::write_str) and
    /// [`read_ieee488_block`](Self::read_ieee488_block).
    pub async fn query_ieee488_block(&mut self, command: &str) -> Result<Vec<u8>> {
        self.write_str(command).await?;
        self.read_ieee488_block().await
    }

    /// Device clear: discards input already received, then sends the
    /// backend's clear command, if any (DESIGN.md S4).
    ///
    /// # Errors
    ///
    /// I/O errors.
    pub async fn clear(&mut self) -> Result<()> {
        self.check_access()?;
        self.buf.clear();
        loop {
            match self.stream.try_read(&mut self.scratch) {
                Ok(0) => return Err(closed()),
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        if let Some(command) = self.settings.clear_command.clone() {
            self.write(&command).await?;
        }
        Ok(())
    }

    /// Reads the status byte with `*STB?`, if 488.2 string emulation is on
    /// (DESIGN.md S5).
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] without emulation; [`Error::InvalidData`] for
    /// a reply that is not a status byte; I/O errors.
    pub async fn read_stb(&mut self) -> Result<u8> {
        if !self.settings.strings {
            return Err(unsupported("read_stb"));
        }
        self.write(b"*STB?\n").await?;
        let reply = self.read_framed(STB_REPLY_MAX, Some(b'\n')).await?;
        if reply.reason != TermReason::TermChar {
            return Err(Error::InvalidData(format!(
                "*STB? reply longer than {STB_REPLY_MAX} bytes"
            )));
        }
        parse_stb(&reply.data)
    }

    /// Sends `*TRG`, if 488.2 string emulation is on (DESIGN.md S5).
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] without emulation; I/O errors.
    pub async fn assert_trigger(&mut self) -> Result<()> {
        if !self.settings.strings {
            return Err(unsupported("assert_trigger"));
        }
        self.write(b"*TRG\n").await.map(drop)
    }

    /// Locks the resource, waiting up to `timeout` for other sessions'
    /// locks (DESIGN.md S6).
    ///
    /// # Errors
    ///
    /// [`Error::ResourceLocked`] if the lock is not granted in time.
    pub async fn lock(&mut self, kind: LockKind, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.locks.try_lock(&self.resource, self.id, &kind) {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(Error::ResourceLocked);
            }
            tokio::time::sleep(LOCK_POLL.min(deadline - now)).await;
        }
    }

    /// Releases one level of lock.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if the session holds no lock.
    pub fn unlock(&mut self) -> Result<()> {
        self.locks.unlock(&self.resource, self.id)
    }
}

impl Drop for AsyncSocketSession {
    fn drop(&mut self) {
        self.locks.release_all(&self.resource, self.id);
    }
}

fn closed() -> Error {
    std::io::Error::new(
        ErrorKind::UnexpectedEof,
        "the instrument closed the connection",
    )
    .into()
}
