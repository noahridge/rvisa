# rvisa-vendor

The vendor VISA backend for `rvisa`: NI-VISA, Keysight VISA, R&S VISA or
any other VPP-4.3 implementation, **loaded at runtime**. Nothing links
against VISA at build time, so binaries start on machines without it and
report `Error::BackendUnavailable` with instructions.

```rust,no_run
use rvisa_core::{Backend, OpenOptions, SessionExt};
use rvisa_vendor::VendorBackend;

let visa = VendorBackend::new();
let mut dmm = visa.open(&"TCPIP0::192.168.1.50::INSTR".parse()?, &OpenOptions::new())?;
println!("{}", dmm.query("*IDN?")?);
# Ok::<(), rvisa_core::Error>(())
```

Applications normally use it through the `rvisa` facade, where it is on by
default (feature `vendor`).

## Finding the library

1. `VendorBackend::with_library_path(...)` (or `ResourceManager::builder().vendor_library(...)`)
2. the `RVISA_VENDOR_LIB` environment variable
3. platform defaults:
   - Windows: `visa64.dll` / `visa32.dll`, then the VXIplug&play directory
     from the registry
   - Linux: `libvisa.so` and `libvisa.so.*`
   - macOS: `/Library/Frameworks/VISA.framework` (NI), then `RsVisa.framework` (R&S)

## Scope

Message-based I/O, attributes, locks, the event queue (SRQ), discovery and
alias resolution. Callback handlers, register access and VISA's async I/O
are out of scope (ROADMAP §3). All `unsafe` code is in one module.
