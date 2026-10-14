# rvisa-socket

A pure-Rust raw TCP socket backend for `rvisa`:
`TCPIP[board]::host::port::SOCKET` resources, such as SCPI on port 5025.
No VISA installation is needed.

```rust,no_run
use rvisa_core::{Backend, OpenOptions, Session, SessionExt};
use rvisa_socket::SocketBackend;

let backend = SocketBackend::new();
let mut dmm = backend.open(&"TCPIP0::192.168.1.50::5025::SOCKET".parse()?, &OpenOptions::new())?;
dmm.set_read_termination(Some(b'\n'))?;
let idn = dmm.query_bytes(b"*IDN?\n")?;
# Ok::<(), rvisa_core::Error>(())
```

Applications normally use it through the `rvisa` facade, where it is on by
default (feature `socket`).

## Behaviour

It matches NI-VISA on the same resources:

- **Reads** end at the term char, if enabled, or after the requested
  count. A raw socket has no end-of-message signal, so enable the term
  char for SCPI instruments. A read that times out returns
  `Error::Timeout` carrying the bytes that did arrive.
- **Writes** send bytes unchanged.
- **Device clear** discards input already received, then optionally sends
  `SocketBackend::clear_command`.
- **Status byte and trigger** are unsupported unless 488.2 string emulation
  is on (`SocketBackend::emulate_488_strings`, or `Attr::Raw(ATTR_IO_PROT)`
  set to 4): then they send `*STB?` and `*TRG`.
- **Locks** work between sessions of one backend in one process. There are
  no events (no SRQ on a raw socket).
- Defaults: 2 s timeout, term char `\n` disabled, `TCP_NODELAY` and
  keepalive on.

## Async

With the `tokio` feature, `rvisa_socket::tokio::AsyncSocketSession` is a
native async session with the same behaviour.
