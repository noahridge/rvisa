# rvisa

Control test and measurement instruments from Rust using the VISA model:
resource strings, sessions, read/write/query, attributes and events.

- **Pluggable backends** behind one `Session` trait. Code does not change
  when a backend is swapped.
- **Today (Phase 0):** the core API, a scripted mock backend, record/replay
  traces and a backend conformance suite.
- **Next:** a vendor VISA backend (NI, Keysight, R&S VISA), loaded at
  runtime so binaries run without VISA installed. After that, pure-Rust
  transports: raw socket, VXI-11, USBTMC, HiSLIP and serial.

> Status: pre-release. Nothing is published yet. The crate names are
> placeholders.

## Quick start (mock backend)

```rust
use rvisa::prelude::*;

fn main() -> rvisa::Result<()> {
    // With the `mock` feature, demo instruments are registered.
    let rm = ResourceManager::new()?;
    let mut dmm = rm.open("TCPIP0::mock.local::5025::SOCKET")?;
    dmm.set_read_termination(Some(b'\n'))?;
    println!("{}", dmm.query("*IDN?")?);
    Ok(())
}
```

```text
cargo run -p rvisa --features mock --example mock_idn
```

## Crates

| Crate | Purpose |
|---|---|
| [`rvisa`](crates/rvisa) | Facade: `ResourceManager`, backend registry and selection |
| [`rvisa-core`](crates/rvisa-core) | `ResourceName`, `Session`/`Backend` traits, `Error`, attributes, IEEE 488.2 helpers |
| [`rvisa-mock`](crates/rvisa-mock) | Scripted mock backend, `RecordingSession`/`ReplaySession` |
| [`rvisa-conformance`](crates/rvisa-conformance) | Backend-agnostic conformance suite |

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

`legacy/` holds an earlier prototype kept as reference for Phase 1. It is
not part of the workspace (see DESIGN.md H1).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
