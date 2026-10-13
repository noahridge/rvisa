# rvisa-mock

A scripted mock backend for `rvisa`, plus `RecordingSession` and
`ReplaySession`, which write and play back JSON Lines traces.

```rust
use rvisa_core::{Backend, OpenOptions, SessionExt};
use rvisa_mock::{MockBackend, Script};

let backend = MockBackend::new().instrument(
    "GPIB0::22::INSTR".parse()?,
    Script::new().on_write("*IDN?\n").respond(b"ACME,PSU,1,1.0\n"),
);
let mut psu = backend.open(&"GPIB0::22".parse()?, &OpenOptions::new())?;
assert_eq!(psu.query_bytes(b"*IDN?\n")?, b"ACME,PSU,1,1.0\n");
# Ok::<(), rvisa_core::Error>(())
```
