# rvisa-vxi11

A pure-Rust VXI-11 backend for `rvisa`:
`TCPIP[board]::host[::device][::INSTR]` resources, the protocol most LXI
instruments speak. No VISA installation is needed.

```rust,no_run
use rvisa_core::{Backend, OpenOptions, SessionExt};
use rvisa_vxi11::Vxi11Backend;

let backend = Vxi11Backend::new();
let mut dmm = backend.open(&"TCPIP0::192.168.1.50::inst0::INSTR".parse()?, &OpenOptions::new())?;
let idn = dmm.query_bytes(b"*IDN?\n")?;
# Ok::<(), rvisa_core::Error>(())
```

Applications normally use it through the `rvisa` facade, where it is on by
default (feature `vxi11`).

## Behaviour

- **Opening** asks the instrument's portmapper for the VXI-11 core
  channel, connects, and creates a link to the device: `inst0` unless the
  resource names another, such as `gpib0,5`. HiSLIP resources
  (`hislip0`) are not handled here.
- **Writes** are split by the device's `maxRecvSize`, with END on the last
  part unless `SendEndEnabled` is off.
- **Reads** end at END, at the term char if enabled, or after the
  requested count. No term char is needed. A read that times out returns
  `Error::Timeout` carrying the bytes that did arrive.
- **Device clear, status byte, trigger** are native VXI-11 calls.
- **Locks** work between sessions in one process, and an exclusive lock
  also locks the device against other clients.
- **Timeouts:** a device that does not answer within the timeout plus a
  second is sent `device_abort` on the abort channel.
  `Vxi11Session::abort_handle` aborts from another thread.
- **Service requests** need the `srq` feature: the session then serves
  the interrupt channel, which the instrument connects back to (allow
  inbound TCP through the firewall).
- **Discovery:** `Vxi11Backend::discover` broadcasts a portmapper query.
  `list()` does so only when `Vxi11Backend::discovery` is set.
- Defaults: 2 s timeout, term char `\n` disabled, END on.

## Features

- `srq`: service request events.
- `server`: `Vxi11Server`, which serves any session as a VXI-11 instrument
  on a local port, for tests.
