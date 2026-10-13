# rvisa

Control test and measurement instruments through VISA-style resources, with
pluggable backends. `ResourceManager` opens a resource with the first
backend, in priority order, that supports it.

```rust
use rvisa::prelude::*;

let rm = ResourceManager::new()?;          // compiled-in backends
let mut dmm = rm.open("TCPIP0::mock.local::5025::SOCKET")?;
dmm.set_read_termination(Some(b'\n'))?;
let idn = dmm.query("*IDN?")?;             // write termination "\n" appended
# Ok::<(), rvisa::Error>(())
```

Features:

- `mock`: registers the `rvisa-mock` demo backend.

See the [workspace README](../../README.md).
