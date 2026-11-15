//! Portmapper lookups (RFC 1833 `GETPORT`) and broadcast discovery
//! (DESIGN.md R3, R8).

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use rvisa_core::Result;

use crate::proto::{
    DEVICE_CORE, DEVICE_VERSION, PMAPPROC_GETPORT, PORTMAP_PROG, PORTMAP_VERS, decode_port,
    encode_getport,
};
use crate::rpc::{RpcClient, decode_reply, encode_call};

/// The largest portmapper reply accepted.
const MAX_REPLY: usize = 1024;

/// Asks the portmapper at `addr` for the TCP port of the VXI-11 core
/// channel. Returns 0 if the program is not registered.
pub(crate) fn core_port(addr: SocketAddr, timeout: Duration) -> Result<u16> {
    let mut client = RpcClient::connect(addr, timeout, MAX_REPLY)?;
    let reply = client.call(
        PORTMAP_PROG,
        PORTMAP_VERS,
        PMAPPROC_GETPORT,
        &encode_getport(DEVICE_CORE, DEVICE_VERSION),
        Some(timeout),
    )?;
    decode_port(&reply)
}

/// Broadcasts a `GETPORT` for the VXI-11 core channel to `targets` over
/// UDP and returns the addresses of the hosts that answer with a port,
/// collected for `wait`.
pub(crate) fn discover(targets: &[SocketAddr], wait: Duration) -> Result<Vec<IpAddr>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;
    let xid = std::process::id() ^ 0x5658_4449;
    let call = encode_call(
        xid,
        PORTMAP_PROG,
        PORTMAP_VERS,
        PMAPPROC_GETPORT,
        &encode_getport(DEVICE_CORE, DEVICE_VERSION),
    );
    for target in targets {
        if let Err(e) = socket.send_to(&call, target) {
            tracing::debug!(%target, error = %e, "vxi11: discovery broadcast failed");
        }
    }
    let deadline = Instant::now() + wait;
    let mut found = BTreeSet::new();
    let mut buf = [0u8; MAX_REPLY];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        socket.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                let port = decode_reply(&buf[..n]).and_then(|(got, result)| {
                    if got == xid {
                        decode_port(result?)
                    } else {
                        Ok(0)
                    }
                });
                match port {
                    Ok(0) => {}
                    Ok(_) => {
                        tracing::debug!(%from, "vxi11: discovered an instrument");
                        found.insert(from.ip());
                    }
                    Err(e) => tracing::debug!(%from, error = %e, "vxi11: bad discovery reply"),
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            // Windows reports an ICMP port-unreachable from an earlier send
            // as a receive error; it does not end the search.
            Err(e) if e.kind() == ErrorKind::ConnectionReset => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(found.into_iter().collect())
}
