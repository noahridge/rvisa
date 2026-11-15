//! The VXI-11 backend: options, opening links, and discovery
//! (DESIGN.md R1, R3, R8).

use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rvisa_core::{
    Backend, Error, LockRegistry, OpenOptions, Pattern, ResourceName, Result, Session,
};

use crate::BACKEND;
use crate::portmap;
use crate::proto::{
    CREATE_LINK, CreateLinkParms, CreateLinkResp, DEVICE_CORE, DEVICE_VERSION, check, code,
};
use crate::rpc::RpcClient;
use crate::session::{Link, Vxi11Session};
use crate::settings::{MAX_READ_REQUEST, Options};

/// The device name used when the resource has none.
pub const DEFAULT_DEVICE: &str = "inst0";

/// The largest reply record the core channel accepts: the largest read
/// plus room for the RPC and VXI-11 headers.
const MAX_REPLY: usize = MAX_READ_REQUEST + 1024;

/// The pure-Rust backend for VXI-11 resources
/// (`TCPIP[board]::host[::device][::INSTR]`, where the device is `inst0`,
/// `gpib0,5` and the like, but not `hislip*`).
///
/// Its defaults match NI-VISA: a 2 s I/O timeout, the term char `\n` but
/// disabled, END sent with the last byte of each write. VXI-11 marks the
/// end of a message, so reads end there without a term char:
///
/// ```no_run
/// use rvisa_core::{Backend, OpenOptions, SessionExt};
/// use rvisa_vxi11::Vxi11Backend;
///
/// let backend = Vxi11Backend::new();
/// let mut dmm = backend.open(&"TCPIP0::192.168.1.50::INSTR".parse()?, &OpenOptions::new())?;
/// let idn = dmm.query_bytes(b"*IDN?\n")?;
/// # Ok::<(), rvisa_core::Error>(())
/// ```
#[derive(Debug, Clone, Default)]
pub struct Vxi11Backend {
    options: Options,
    locks: Arc<LockRegistry>,
}

impl Vxi11Backend {
    /// A backend with the default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How long `open` waits for each connection and for `create_link`
    /// (5 s by default).
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.options.connect_timeout = timeout;
        self
    }

    /// The portmapper's TCP and UDP port (111 by default). Other values are
    /// for tunnels and test servers.
    #[must_use]
    pub fn portmapper_port(mut self, port: u16) -> Self {
        self.options.portmapper_port = port;
        self
    }

    /// Makes [`list`](Backend::list) search the network: it broadcasts a
    /// portmapper query and collects answers for `wait` (DESIGN.md R8).
    /// Off by default, so listing stays fast.
    #[must_use]
    pub fn discovery(mut self, wait: Option<Duration>) -> Self {
        self.options.discovery = wait;
        self
    }

    /// Where discovery sends its query, as `ip:port`. By default it goes to
    /// the limited broadcast address `255.255.255.255` on the portmapper
    /// port; some networks need a subnet's directed broadcast address
    /// instead (such as `192.168.1.255:111`).
    #[must_use]
    pub fn discovery_targets(mut self, targets: impl IntoIterator<Item = SocketAddr>) -> Self {
        self.options.discovery_targets = targets.into_iter().collect();
        self
    }

    /// Searches the network for VXI-11 instruments for `wait`, whatever
    /// [`discovery`](Self::discovery) is set to. Each answer becomes
    /// `TCPIP0::<address>::inst0::INSTR`.
    ///
    /// # Errors
    ///
    /// I/O errors from the UDP socket.
    pub fn discover(&self, wait: Duration) -> Result<Vec<ResourceName>> {
        let targets = if self.options.discovery_targets.is_empty() {
            vec![SocketAddr::from((
                Ipv4Addr::BROADCAST,
                self.options.portmapper_port,
            ))]
        } else {
            self.options.discovery_targets.clone()
        };
        Ok(portmap::discover(&targets, wait)?
            .into_iter()
            .map(|ip| ResourceName::TcpipInstr {
                board: 0,
                host: ip.to_string(),
                device: Some(DEFAULT_DEVICE.to_owned()),
            })
            .collect())
    }

    /// Opens a session as the concrete [`Vxi11Session`].
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] for a resource this backend does not
    /// support; [`Error::NotFound`] if the host does not resolve, refuses
    /// the connection, has no VXI-11 service, or has no such device;
    /// [`Error::Timeout`] if connecting takes longer than
    /// [`connect_timeout`](Self::connect_timeout); [`Error::ResourceLocked`]
    /// if a lock requested in `options` is not granted in time; other I/O
    /// and protocol errors.
    pub fn open_vxi11(
        &self,
        resource: &ResourceName,
        options: &OpenOptions,
    ) -> Result<Vxi11Session> {
        let Some((host, device)) = link_target(resource) else {
            return Err(Error::InvalidArgument(format!(
                "{resource} is not a VXI-11 resource"
            )));
        };
        let timeout = self.options.connect_timeout;
        let mut core = self.connect(resource, host)?;
        let parms = CreateLinkParms {
            client_id: i32::try_from(std::process::id() & 0x7FFF_FFFF).unwrap_or_default(),
            lock_device: false,
            lock_timeout: 0,
            device: device.to_owned(),
        };
        tracing::debug!(%resource, device, "vxi11: create_link");
        let reply = core.call(
            DEVICE_CORE,
            DEVICE_VERSION,
            CREATE_LINK,
            &parms.encode(),
            Some(timeout),
        )?;
        let resp = CreateLinkResp::decode(&reply)?;
        match resp.error {
            code::INVALID_ADDRESS | code::NOT_ACCESSIBLE => {
                return Err(Error::NotFound(resource.clone()));
            }
            error => check(error, "create_link")?,
        }
        let peer = core.stream().peer_addr()?;
        let link = Link {
            lid: resp.lid,
            abort_addr: SocketAddr::new(peer.ip(), resp.abort_port),
            max_recv_size: (resp.max_recv_size as usize).clamp(1, MAX_READ_REQUEST),
        };
        tracing::debug!(%resource, lid = link.lid, max_recv_size = link.max_recv_size, "vxi11: linked");
        let mut session = Vxi11Session::new(core, resource.clone(), link, Arc::clone(&self.locks));
        if let Some(kind) = &options.lock {
            rvisa_core::Session::lock(&mut session, kind.clone(), options.open_timeout)?;
        }
        Ok(session)
    }

    /// Finds the core channel through the portmapper on each address of
    /// `host` in turn and connects to it.
    fn connect(&self, resource: &ResourceName, host: &str) -> Result<RpcClient> {
        let timeout = self.options.connect_timeout;
        let addrs: Vec<SocketAddr> = (host, self.options.portmapper_port)
            .to_socket_addrs()
            .map_err(|e| {
                tracing::debug!(%resource, error = %e, "vxi11: cannot resolve host");
                Error::NotFound(resource.clone())
            })?
            .collect();
        let mut last = None;
        for addr in addrs {
            tracing::debug!(%resource, %addr, "vxi11: portmapper GETPORT");
            let attempt = portmap::core_port(addr, timeout).and_then(|port| {
                if port == 0 {
                    return Err(Error::NotFound(resource.clone()));
                }
                RpcClient::connect(SocketAddr::new(addr.ip(), port), timeout, MAX_REPLY)
            });
            match attempt {
                Ok(client) => return Ok(client),
                Err(e) => {
                    tracing::debug!(%resource, %addr, error = %e, "vxi11: cannot connect");
                    last = Some(e);
                }
            }
        }
        Err(match last {
            Some(Error::Io(e)) if e.kind() == ErrorKind::ConnectionRefused => {
                Error::NotFound(resource.clone())
            }
            Some(e) => e,
            None => Error::NotFound(resource.clone()),
        })
    }
}

/// The host and VXI-11 device name of a resource this backend supports.
fn link_target(resource: &ResourceName) -> Option<(&str, &str)> {
    let ResourceName::TcpipInstr { host, device, .. } = resource else {
        return None;
    };
    let device = device.as_deref().unwrap_or(DEFAULT_DEVICE);
    let hislip = device
        .get(..6)
        .is_some_and(|p| p.eq_ignore_ascii_case("hislip"));
    (!hislip).then_some((host, device))
}

impl Backend for Vxi11Backend {
    fn name(&self) -> &'static str {
        BACKEND
    }

    fn supports(&self, resource: &ResourceName) -> bool {
        link_target(resource).is_some()
    }

    fn open(&self, resource: &ResourceName, options: &OpenOptions) -> Result<Box<dyn Session>> {
        Ok(Box::new(self.open_vxi11(resource, options)?))
    }

    /// Empty unless [`discovery`](Vxi11Backend::discovery) is on; then the
    /// instruments that answer a broadcast, filtered by `pattern`.
    fn list(&self, pattern: &str) -> Result<Vec<ResourceName>> {
        let pattern = Pattern::new(pattern)?;
        let Some(wait) = self.options.discovery else {
            return Ok(Vec::new());
        };
        Ok(self
            .discover(wait)?
            .into_iter()
            .filter(|rn| pattern.matches(rn))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rn(s: &str) -> ResourceName {
        s.parse().unwrap()
    }

    #[test]
    fn supported_resources() {
        let b = Vxi11Backend::new();
        assert!(b.supports(&rn("TCPIP::10.0.0.1::INSTR")));
        assert!(b.supports(&rn("TCPIP0::h::inst0::INSTR")));
        assert!(b.supports(&rn("TCPIP0::h::gpib0,5::INSTR")));
        assert!(!b.supports(&rn("TCPIP0::h::hislip0::INSTR")));
        assert!(!b.supports(&rn("TCPIP0::h::HiSLIP1::INSTR")));
        assert!(!b.supports(&rn("TCPIP0::h::5025::SOCKET")));
        assert!(!b.supports(&rn("GPIB0::22::INSTR")));
        assert_eq!(link_target(&rn("TCPIP::h::INSTR")), Some(("h", "inst0")));
        assert!(matches!(
            b.open_vxi11(&rn("TCPIP0::h::hislip0::INSTR"), &OpenOptions::new()),
            Err(Error::InvalidArgument(_))
        ));
    }

    #[test]
    fn list_is_empty_without_discovery() {
        let b = Vxi11Backend::new();
        assert!(b.list("?*").unwrap().is_empty());
        assert!(b.list("[").is_err());
    }
}
