//! A scripted instrument served on a local TCP socket (DESIGN.md M6).

use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rvisa_core::{Backend, OpenOptions, ResourceName, Session};

use crate::{MockBackend, Script};

const POLL: Duration = Duration::from_millis(5);

/// A [`Script`] served on `127.0.0.1` with raw-socket semantics: each
/// newline-terminated command is matched against the script, and responses
/// are sent back as bytes, with no END indicator.
///
/// With a real VISA installed, the [`resource`](Self::resource) name
/// (`TCPIP0::127.0.0.1::<port>::SOCKET`) reaches it like an instrument. The
/// server stops when this value is dropped.
///
/// ```
/// use std::io::{BufRead, BufReader, Write};
/// use rvisa_mock::{Script, TcpInstrument};
///
/// let server = TcpInstrument::spawn(Script::new().on_write("*IDN?\n").respond(b"ACME,1,2,3\n"))?;
/// let mut stream = std::net::TcpStream::connect(server.addr())?;
/// stream.write_all(b"*IDN?\n")?;
/// let mut line = String::new();
/// BufReader::new(stream).read_line(&mut line)?;
/// assert_eq!(line, "ACME,1,2,3\n");
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct TcpInstrument {
    addr: SocketAddr,
    resource: ResourceName,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl TcpInstrument {
    /// Starts serving `script` on a free local port.
    ///
    /// # Errors
    ///
    /// If the socket cannot be bound.
    pub fn spawn(script: Script) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let resource: ResourceName = format!("TCPIP0::127.0.0.1::{}::SOCKET", addr.port())
            .parse()
            .map_err(io::Error::other)?;
        let backend = Arc::new(MockBackend::new().instrument(resource.clone(), script));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (stop, resource) = (Arc::clone(&stop), resource.clone());
            thread::spawn(move || accept_loop(&listener, &backend, &resource, &stop))
        };
        Ok(Self {
            addr,
            resource,
            stop,
            thread: Some(thread),
        })
    }

    /// The listening address.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The VISA resource name for this server.
    #[must_use]
    pub fn resource(&self) -> &ResourceName {
        &self.resource
    }
}

impl Drop for TcpInstrument {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn accept_loop(
    listener: &TcpListener,
    backend: &Arc<MockBackend>,
    resource: &ResourceName,
    stop: &Arc<AtomicBool>,
) {
    let mut connections = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                tracing::debug!(%peer, "mock tcp: connection");
                let (backend, resource, stop) =
                    (Arc::clone(backend), resource.clone(), Arc::clone(stop));
                connections.push(thread::spawn(move || {
                    if let Err(e) = serve(stream, &backend, &resource, &stop) {
                        tracing::debug!(error = %e, "mock tcp: connection ended");
                    }
                }));
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(e) => {
                tracing::debug!(error = %e, "mock tcp: accept failed");
                break;
            }
        }
    }
    for connection in connections {
        let _ = connection.join();
    }
}

/// Pumps one connection: commands in, scripted responses out.
fn serve(
    mut stream: TcpStream,
    backend: &MockBackend,
    resource: &ResourceName,
    stop: &AtomicBool,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(POLL))?;
    stream.set_nodelay(true)?;
    let mut session = backend
        .open(resource, &OpenOptions::new())
        .map_err(io::Error::other)?;
    session
        .set_timeout(Some(Duration::from_millis(1)))
        .map_err(io::Error::other)?;
    let mut pending = Vec::new();
    let mut buf = [0u8; 4096];
    while !stop.load(Ordering::Relaxed) {
        match stream.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => pending.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(e),
        }
        while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
            let command: Vec<u8> = pending.drain(..=pos).collect();
            session.write(&command).map_err(io::Error::other)?;
        }
        // Send whatever responses are ready.
        while let Ok(read) = session.read(64 * 1024) {
            stream.write_all(&read.data)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    #[cfg_attr(miri, ignore = "Miri does not support sockets")]
    fn serves_scripts_over_tcp() {
        let script = Script::new()
            .on_write("*IDN?\n")
            .respond(b"ACME,TCP,1,1.0\n")
            .on_write("SLOW?\n")
            .delay(Duration::from_millis(30))
            .respond(b"late\n");
        let server = TcpInstrument::spawn(script).unwrap();
        assert_eq!(
            server.resource().to_string(),
            format!("TCPIP0::127.0.0.1::{}::SOCKET", server.addr().port())
        );

        let mut stream = TcpStream::connect(server.addr()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        // Two commands in one packet, one split across packets.
        stream.write_all(b"*IDN?\nSLOW?\n*ID").unwrap();
        stream.write_all(b"N?\n").unwrap();
        let mut reader = BufReader::new(stream);
        let mut lines = Vec::new();
        for _ in 0..3 {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            lines.push(line);
        }
        // Responses leave in order, so the delayed one holds back the next.
        assert_eq!(lines, ["ACME,TCP,1,1.0\n", "late\n", "ACME,TCP,1,1.0\n"]);
        drop(server);
    }
}
