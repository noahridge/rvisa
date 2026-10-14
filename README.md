# rvisa

Control test and measurement instruments from Rust using the VISA model:
resource strings, sessions, read/write/query, attributes and events.

- **Pluggable backends** behind one `Session` trait. Code does not change
  when a backend is swapped.
- **Today:** a vendor VISA backend (NI, Keysight, R&S VISA), loaded at
  runtime so binaries run without VISA installed. Also the core API, a
  scripted mock backend, record/replay traces, a backend conformance suite,
  and an async adapter for tokio.
- **Next:** pure-Rust transports: raw socket, VXI-11, USBTMC, HiSLIP and
  serial.

> Status: pre-release. Nothing is published yet. The crate names are
> placeholders.

## Quick start

```rust
use rvisa::prelude::*;

fn main() -> rvisa::Result<()> {
    let rm = ResourceManager::new()?;              // vendor VISA, loaded at runtime
    for name in rm.list("?*INSTR")? {
        println!("{name}");
    }
    let mut dmm = rm.open("TCPIP0::192.168.1.50::INSTR")?;
    println!("{}", dmm.query("*IDN?")?);           // "\n" write termination appended
    Ok(())
}
```

Examples (they need a VISA and an instrument, except `mock_idn`):

```text
cargo run -p rvisa --example list_resources
cargo run -p rvisa --example idn -- TCPIP0::192.168.1.50::INSTR
cargo run -p rvisa --example binary_block_capture -- RESOURCE CURV? curve.bin
cargo run -p rvisa --example srq_wait -- GPIB0::22::INSTR
cargo run -p rvisa --features mock --example mock_idn      # no hardware
```

Without VISA installed, they exit with a message saying what to install,
or that `RVISA_VENDOR_LIB` can point to the library.

## Crates

| Crate | Purpose |
|---|---|
| [`rvisa`](crates/rvisa) | Facade: `ResourceManager`, backend registry and selection |
| [`rvisa-core`](crates/rvisa-core) | `ResourceName`, `Session`/`Backend` traits, `Error`, attributes, IEEE 488.2 helpers |
| [`rvisa-vendor`](crates/rvisa-vendor) | Vendor VISA backend, loaded at runtime |
| [`rvisa-mock`](crates/rvisa-mock) | Scripted mock backend, `RecordingSession`/`ReplaySession`, `TcpInstrument` |
| [`rvisa-conformance`](crates/rvisa-conformance) | Backend-agnostic conformance suite |
| `rvisa-vendor-fake` | Test-only fake VISA library over the mock (not published) |

## Documents

- [ROADMAP.md](docs/ROADMAP.md): the plan, phases and binding decisions
- [DESIGN.md](docs/DESIGN.md): design notes and deviations from the roadmap
- [HARDWARE.md](docs/HARDWARE.md): the hardware test matrix
- [CHANGELOG.md](CHANGELOG.md)

## Development

The toolchain is pinned to the MSRV (1.97) in `rust-toolchain.toml`.

```text
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check
cargo +nightly fuzz run resource_name -- -max_total_time=60   # in fuzz/
```

Tests run the vendor backend against `rvisa-vendor-fake`, a fake VISA
shared library, so no VISA is needed. Tests against real VISA and
instruments are `#[ignore]`d; see [HARDWARE.md](docs/HARDWARE.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
