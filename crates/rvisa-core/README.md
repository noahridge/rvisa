# rvisa-core

Core types for `rvisa`: `ResourceName`, the `Session`, `SessionExt` and
`Backend` traits, `Error`, attributes, events, VISA search patterns and an
IEEE 488.2 block parser. Backend crates depend on this; applications use
the `rvisa` facade.

```rust
use rvisa_core::ResourceName;

let rn: ResourceName = "tcpip::[fe80::1]::hislip0".parse()?;
assert_eq!(rn.to_string(), "TCPIP0::[fe80::1]::hislip0::INSTR");
# Ok::<(), rvisa_core::Error>(())
```
