# Rust VISA Library: Plan & Roadmap

> **Handoff document for an implementation agent.**
> Read the whole document before starting. Work one phase at a time, and stop for human review at the end of each phase (see §9).

---

## 1. Summary

Build a Rust library for controlling test & measurement instruments using the VISA model: resource strings, sessions, read/write/query, attributes, and events.

- **Start:** a clean, safe, idiomatic wrapper around the vendor VISA library (NI-VISA, Keysight VISA, R&S VISA). The vendor library is loaded **at runtime**.
- **End state:** a full **pure-Rust** implementation of the common transports (raw socket, VXI-11, USBTMC, HiSLIP, serial). The vendor library becomes an optional fallback.

The key architectural rule is that **the vendor wrapper is just the first backend behind a transport-agnostic `Session` trait.** Users must never have to change their code when a pure-Rust backend replaces the vendor one.

**Working crate name:** `rvisa`. This is a placeholder. Check availability on crates.io before any publish. Do **not** use "NI" or "ni-visa" in any crate name (trademark).

---

## 2. Context: why this exists

The existing ecosystem was reviewed in October 2026:

- **`visa-rs`** (with `visa-sys` and `visa-rs-proc`) is the only maintained safe wrapper. Its limitations:
  - It links to VISA **at build time**, configured through the `LIB_VISA_NAME` and `LIB_VISA_PATH` environment variables.
  - API coverage is incomplete.
  - The API is shaped like C (`CString` resource expressions, hand-mapped I/O errors).
  - Async I/O is reported to block until timeout on macOS with NI-VISA 23.5.
  - It has a single maintainer.
- **`instrument-components`** puts typed IVI-style classes on top of `visa-rs`. It is new (2026) and still depends on vendor VISA.
- **Pure-Rust attempts** (`instrument-ctl`, `rust-usbtmc`, `rscpi`, `usbtmc`) are partial or abandoned. `lxi-rs` implements the *device/server* side, not the client.
- **Nobody has built a pyvisa-py equivalent in Rust.** That is the gap this project fills.

What makes this library different from day one:

1. Runtime loading of the vendor library, so one binary runs with or without VISA installed.
2. An idiomatic Rust API: a typed resource parser, a real error enum, and typed attributes.
3. A mock and record/replay backend for CI without hardware.
4. A backend conformance suite, so pure-Rust backends are *proven* equivalent to vendor VISA.

---

## 3. Goals and non-goals

### Goals
- A safe Rust API for **message-based** instruments: `INSTR` over TCPIP, USB, GPIB (vendor backend only), and ASRL, plus `SOCKET`.
- Runtime-selectable backends, with a defined priority order and an explicit override.
- Sync API first. Async (Tokio) comes later and is done natively in the pure-Rust backends.
- Linux, Windows, and macOS. Cross-compiles to ARM Linux (aarch64, armv7).
- Long term: Python bindings usable as a **pyvisa backend** (Phase 7).

### Non-goals (unless explicitly requested later)
- Register-based access, PXI, VXI memory maps (`viIn*`, `viOut*`, `viMove*`, `viMapAddress`).
- Callback handlers via `viInstallHandler`. Use the event *queue* mechanism instead.
- A pure-Rust GPIB backend. GPIB stays on the vendor backend, and optionally on `linux-gpib-rs` later.
- Instrument drivers or SCPI command abstractions such as typed DMM or PSU classes. Those belong in downstream crates.
- Vendor `viReadAsync` and `viWriteAsync`.

---

## 4. Workspace layout

```
rvisa/
├── Cargo.toml                 # workspace
├── crates/
│   ├── rvisa/                 # facade: ResourceManager, backend registry/selection, re-exports
│   ├── rvisa-core/            # ResourceName, Session trait, Error, attributes, 488.2 helpers
│   ├── rvisa-vendor/          # runtime-loaded vendor VISA backend (NI/Keysight/R&S)
│   ├── rvisa-mock/            # scripted mock + record/replay backend
│   ├── rvisa-conformance/     # backend-agnostic test suite (lib, used by every backend's tests)
│   ├── rvisa-socket/          # Phase 2: TCPIP::host::port::SOCKET
│   ├── rvisa-vxi11/           # Phase 3: ONC-RPC VXI-11
│   ├── rvisa-usbtmc/          # Phase 4: USBTMC/USB488 via nusb
│   ├── rvisa-hislip/          # Phase 5: HiSLIP
│   └── rvisa-serial/          # Phase 6: ASRL via serialport
├── examples/
├── traces/                    # recorded golden sessions (see §7.3)
├── docs/
│   ├── DESIGN.md              # living design notes; record every decision that deviates from this plan
│   └── HARDWARE.md            # hardware test matrix
└── .github/workflows/
```

Only create a crate when its phase begins. Phase 0 creates `rvisa`, `rvisa-core`, `rvisa-mock`, and `rvisa-conformance`.

---

## 5. Core API (target shape)

This is a sketch, not final code. Adjust names as needed, but keep the structure. Record any deviation in `docs/DESIGN.md`.

```rust
// ---------- rvisa-core ----------

/// Parsed VISA resource string. Must round-trip: parse(format(x)) == x.
#[non_exhaustive]
pub enum ResourceName {
    TcpipInstr  { board: u16, host: String, device: Option<String> }, // device: inst0 / hislip0 / ...
    TcpipSocket { board: u16, host: String, port: u16 },
    UsbInstr    { board: u16, vid: u16, pid: u16, serial: String, interface: Option<u16> },
    GpibInstr   { board: u16, primary: u8, secondary: Option<u8> },
    AsrlInstr   { board: u16 },          // also accept "COM3" / "/dev/ttyUSB0" aliases
    // Unknown/unsupported forms are a parse error, never a panic.
}
impl std::str::FromStr for ResourceName { /* case-insensitive, VISA-spec compliant */ }
impl std::fmt::Display for ResourceName { /* canonical form */ }

/// One open connection to an instrument. Implemented by every backend.
pub trait Session: Send {
    fn resource(&self) -> &ResourceName;

    fn write(&mut self, data: &[u8]) -> Result<usize>;
    /// Reads until a termination condition (term char / END / EOI / count) or timeout.
    fn read(&mut self, max: usize) -> Result<ReadResult>; // ReadResult { data: Vec<u8>, reason: TermReason }

    fn set_timeout(&mut self, t: Option<Duration>) -> Result<()>;   // None = infinite
    fn timeout(&self) -> Option<Duration>;
    fn set_read_termination(&mut self, term: Option<u8>) -> Result<()>;
    fn set_write_termination(&mut self, term: Option<&[u8]>) -> Result<()>; // appended by the facade, not the backend

    fn clear(&mut self) -> Result<()>;                 // device clear
    fn read_stb(&mut self) -> Result<u8>;              // serial poll
    fn assert_trigger(&mut self) -> Result<()>;
    fn lock(&mut self, kind: LockKind, timeout: Duration) -> Result<()>;
    fn unlock(&mut self) -> Result<()>;

    fn get_attr(&self, attr: Attr) -> Result<AttrValue>;
    fn set_attr(&mut self, attr: Attr, value: AttrValue) -> Result<()>;

    fn enable_event(&mut self, ev: EventKind) -> Result<()>;   // queue mechanism only
    fn wait_event(&mut self, ev: EventKind, timeout: Duration) -> Result<Event>;
    fn disable_event(&mut self, ev: EventKind) -> Result<()>;
}

/// Convenience methods live on an extension trait / wrapper, NOT on backends.
pub trait SessionExt: Session {
    fn query(&mut self, cmd: &str) -> Result<String>;
    fn query_bytes(&mut self, cmd: &[u8]) -> Result<Vec<u8>>;
    fn read_ieee488_block(&mut self) -> Result<Vec<u8>>;     // #<n><len><data>, plus #0 indefinite
    fn query_ieee488_block(&mut self, cmd: &str) -> Result<Vec<u8>>;
}

pub trait Backend: Send + Sync {
    fn name(&self) -> &'static str;                          // "vendor", "socket", "vxi11", ...
    fn supports(&self, rn: &ResourceName) -> bool;
    fn open(&self, rn: &ResourceName, opts: &OpenOptions) -> Result<Box<dyn Session>>;
    fn list(&self, pattern: &str) -> Result<Vec<ResourceName>>;   // may return empty
}

#[non_exhaustive]
pub enum Error {
    Timeout,
    NotFound(ResourceName),
    Io(std::io::Error),
    Protocol { backend: &'static str, detail: String },
    Vendor { status: i32, name: Option<&'static str>, desc: String }, // VISA ViStatus mapping
    Unsupported { backend: &'static str, op: &'static str },
    InvalidResource(String),
    BackendUnavailable { backend: &'static str, reason: String },     // e.g. VISA lib not found
    // ...
}
```

```rust
// ---------- rvisa (facade) ----------
let rm = ResourceManager::new()?;                     // auto: registered backends in priority order
let rm = ResourceManager::builder()
    .prefer(&["socket", "vxi11", "vendor"])           // explicit priority
    .build()?;
let mut dmm = rm.open("TCPIP0::192.168.1.50::5025::SOCKET")?;
dmm.set_read_termination(Some(b'\n'))?;
let idn = dmm.query("*IDN?")?;
```

**Backend selection rule:** the first backend in priority order whose `supports()` returns true opens the session. The user can force a backend per call with `rm.open_with("...", "vendor")`. Default priority, which the user can configure:
- Phase 1: `vendor` only.
- From Phase 6: pure-Rust backends that pass conformance, then `vendor` as the fallback.

---

## 6. Key design decisions (binding)

| # | Decision | Rationale |
|---|---|---|
| D1 | Load the vendor VISA library **at runtime** with `libloading`. Never link it at build time. | One binary works with or without VISA; clean cross-compiles; vendor selection. |
| D2 | Search paths per OS (Windows `visa64.dll`/`visa32.dll` via system path and registry; Linux `libvisa.so`/`libvisa.so.*` in standard paths; macOS VISA framework), plus an `RVISA_VENDOR_LIB` env override and an explicit path in the builder. | Predictable, overridable. |
| D3 | Hand-write the FFI declarations for the roughly 40 functions needed, from the IVI VPP-4.3 spec. Do **not** vendor or commit `visa.h` or bindgen output generated from it. | Avoids header licensing questions; keeps the surface minimal. |
| D4 | Resolve each function pointer at load time into a struct of `Option<fn>`. A missing optional symbol gives `Error::Unsupported` and must not cause a load failure. | Different vendors and versions export different sets. |
| D5 | Sessions hold an `Arc` to the shared backend/RM state. Dropping the RM never invalidates live sessions; the vendor RM closes when the last reference drops. | Avoids the lifetime problems of the visa-rs design. |
| D6 | `Session: Send`, not `Sync`. Mutating I/O uses `&mut self`. | Simple, sound concurrency model. |
| D7 | Events use the queue only (`viEnableEvent(VI_QUEUE)` + `viWaitOnEvent`). No `viInstallHandler`. | C callbacks into Rust are the riskiest FFI surface; the queue covers SRQ. |
| D8 | The vendor backend is **sync only**. Async comes through a `spawn_blocking` adapter in the facade (feature `tokio`). Pure-Rust backends implement async natively later. | Vendor async is unreliable; native async belongs where we own the sockets. |
| D9 | Write termination is appended by the facade layer, not by individual backends. Read termination is a backend concern, because protocols signal END differently. | Consistent behavior across backends. |
| D10 | All `unsafe` lives in `rvisa-vendor`, in one module per concern. Every `unsafe` block has a `// SAFETY:` comment. `#![deny(unsafe_code)]` everywhere else. | Auditable. |
| D11 | Map every VISA status code to a name and description. Positive "warning/success" completion codes (for example `VI_SUCCESS_TERM_CHAR`, `VI_SUCCESS_MAX_CNT`) are surfaced in `ReadResult::reason`, not discarded. | Fixes a known visa-rs gap. |
| D12 | No panics on any input from devices or users. Fuzz the resource parser and the protocol decoders. | Instrument and network input is untrusted. |

---

## 7. Testing strategy

### 7.1 Unit tests
Every crate. The resource parser needs exhaustive cases: every documented form, mixed case, whitespace, aliases, IPv6 hosts in brackets, and invalid inputs.

### 7.2 Conformance suite (`rvisa-conformance`)
A library of backend-agnostic test functions, for example:

```rust
pub fn idn_roundtrip(s: &mut dyn Session) { ... }
pub fn timeout_is_respected(s: &mut dyn Session) { ... }
pub fn read_termination_char(s: &mut dyn Session) { ... }
pub fn binary_block_definite_length(s: &mut dyn Session) { ... }
pub fn large_transfer_1mb(s: &mut dyn Session) { ... }
pub fn device_clear_recovers_after_partial_read(s: &mut dyn Session) { ... }
pub fn stb_and_srq_event(s: &mut dyn Session) { ... }
```

Every backend runs the full suite in its own tests:
- **Mock backend:** always runs, in CI.
- **Hardware:** under `#[ignore]`, gated by env vars such as `RVISA_HW_SOCKET=TCPIP0::...::SOCKET`.

A test that a backend legitimately can't support must return `Unsupported`. It must never silently pass.

### 7.3 Record / replay (golden traces)
- `rvisa-mock` provides a `RecordingSession` wrapper. It logs every call, argument, result, and timing to a JSON Lines trace.
- It also provides a `ReplaySession` that serves a trace back.
- Traces are recorded against the **vendor** backend on real hardware and committed under `traces/<instrument>/<scenario>.jsonl`.
- **Exit criterion for each pure-Rust backend:** on the same hardware, it produces byte-identical instrument responses to the vendor trace for every scenario. Timing is excluded.

### 7.4 Fuzzing
Use `cargo-fuzz` targets for:
- the resource parser
- the IEEE 488.2 block parser
- VXI-11 XDR decoding
- the HiSLIP message decoder
- the USBTMC header decoder

### 7.5 CI
GitHub Actions matrix: `ubuntu-latest`, `windows-latest`, `macos-latest`. Each job runs:
- `fmt`, `clippy -D warnings`, `test` (mock only)
- `cargo-deny` (licenses and advisories)
- a docs build
- a cross-compile check for `aarch64-unknown-linux-gnu` and `armv7-unknown-linux-gnueabihf`
- MSRV check

Hardware tests run on a self-hosted runner, added later.

---

## 8. Phased roadmap

Each phase lists **tasks**, **exit criteria** (all required), and **stop point**. Do not start the next phase until the human approves.

### Phase 0: Foundations
**Tasks**
1. Workspace scaffold, licensing (MIT OR Apache-2.0), README, `docs/DESIGN.md`, `docs/HARDWARE.md` (template only), CI workflow.
2. `rvisa-core`:
   - `ResourceName` (parse/format/round-trip), `Session`, `SessionExt`, `Backend`, `Error`, `Attr`/`AttrValue`, `EventKind`, `ReadResult`/`TermReason`, `OpenOptions`.
   - The IEEE 488.2 definite- and indefinite-length block reader.
3. `rvisa-mock`:
   - A scripted session (builder: `on_write("*IDN?\n").respond(b"ACME,...\n")`, with injected delays and timeouts).
   - `RecordingSession` and `ReplaySession`.
4. `rvisa-conformance`: initial suite, run against the mock.
5. `rvisa` facade: `ResourceManager` with backend registry and priority selection, mock backend registered behind the feature `mock`.

**Exit criteria**
- `cargo test --workspace` is green on all three OS in CI.
- The resource parser has ≥ 95% line coverage and a fuzz target that has run for at least 10 minutes without findings.
- The mock passes the full conformance suite.
- Public API docs exist for every public item (`#![deny(missing_docs)]` in core).

**Stop:** present the API surface (`cargo doc` output plus a summary) for review.

### Phase 1: Vendor backend → release 0.1
**Tasks**
1. `rvisa-vendor`:
   - Library discovery and loading (D1, D2, D4).
   - Hand-written FFI types and declarations (D3).
   - Status-code table (D11).
2. Functions in scope:
   - `viOpenDefaultRM`, `viFindRsrc`, `viFindNext`, `viParseRsrcEx`
   - `viOpen`, `viClose`
   - `viRead`, `viWrite`
   - `viGetAttribute`, `viSetAttribute`
   - `viClear`, `viReadSTB`, `viAssertTrigger`
   - `viLock`, `viUnlock`
   - `viEnableEvent`, `viDisableEvent`, `viWaitOnEvent`, `viDiscardEvents`
   - `viStatusDesc`, `viFlush`
   - GPIB control functions only where trivial.
3. Typed attributes for the common set: timeout, term char and enable, send END, suppress END, serial baud/parity/stop/flow, TCPIP keepalive/nodelay, interface type, manufacturer/model/serial (USB), and resource name. Any other attribute is reachable through a raw `Attr::Raw(u32)` escape hatch.
4. `list()` via `viFindRsrc`, mapped into `ResourceName`.
5. A `tokio` feature in the facade: an async wrapper using `spawn_blocking` (D8).
6. Examples: `list_resources`, `idn`, `binary_block_capture`, `srq_wait`.
7. Run the conformance suite against real hardware. Record golden traces (§7.3) for every instrument in `docs/HARDWARE.md`.

**Exit criteria**
- The vendor backend passes conformance on hardware on **Windows and Linux** (macOS is best-effort; document results).
- A binary built **without** VISA installed starts, and returns `BackendUnavailable` cleanly with a helpful message.
- No `unsafe` outside `rvisa-vendor`. All `unsafe` blocks carry `SAFETY` comments. Miri runs on the non-FFI parts.
- Golden traces are committed for at least one TCPIP instrument and one USB instrument.
- CHANGELOG, README quick start, and docs.rs-ready metadata are in place. **Do not publish.** The human publishes.

**Stop:** review before the 0.1 publish.

### Phase 2: Raw socket backend (`TCPIP::host::port::SOCKET`)
**Tasks**
1. A `std::net` implementation with timeouts, `TCP_NODELAY`, optional keepalive, and a configurable read termination (no END signal exists on raw sockets).
2. `clear` is defined as: drain the input buffer, then optionally send a configurable clear string. `read_stb` returns `Unsupported` unless an `*STB?` emulation option is enabled. Document both behaviors.
3. A native async implementation behind the feature `tokio`.

**Exit criteria**
- Passes conformance and **replays golden traces identically** against the same instrument(s). Priority target: the Keithley 3700A on port 5025.
- Throughput is within 10% of the vendor backend on a 1 MB binary transfer (record numbers in `docs/HARDWARE.md`).

**Stop:** review.

### Phase 3: VXI-11 (`TCPIP::host[::inst0]::INSTR`)
**Tasks**
1. Minimal ONC-RPC/XDR client (portmapper `GETPORT`, record marking) in-crate. Evaluate existing XDR crates first, and document the choice in `DESIGN.md`.
2. Core channel: `create_link`, `device_write` (with END flag and chunking by `maxRecvSize`), `device_read` (reason bits → `TermReason`), `device_readstb`, `device_trigger`, `device_clear`, `device_lock`/`device_unlock`, `destroy_link`.
3. Abort channel: `device_abort`.
4. Optional, behind the feature `srq`: interrupt channel (`create_intr_chan`, `device_enable_srq`) with an RPC server for SRQ delivery.
5. Discovery: portmapper broadcast on the local subnet.

**Exit criteria**
- Conformance and golden-trace replay pass against at least two instruments from **different vendors**.
- Fuzzed XDR decoder.

**Stop:** review.

### Phase 4: USBTMC / USB488 (`USB::vid::pid::serial::INSTR`)
**Tasks**
1. Use `nusb` for transport: device enumeration, finding the USBTMC interface (class `0xFE`, subclass `0x03`), and bulk-IN, bulk-OUT, and interrupt-IN endpoints.
2. Implement the USBTMC message headers:
   - `DEV_DEP_MSG_OUT` and `REQUEST_DEV_DEP_MSG_IN`, with `bTag` cycling, the EOM bit, and the term char via `bmTransferAttributes`.
   - Control requests: `INITIATE_CLEAR` and `CHECK_CLEAR_STATUS`, `INITIATE_ABORT_BULK_OUT`/`_IN` with status checks, `GET_CAPABILITIES`.
   - USB488: `READ_STATUS_BYTE`, `TRIGGER`, `REN_CONTROL`, `GO_TO_LOCAL`, `LOCAL_LOCKOUT`.
   - SRQ via interrupt-IN.
3. Robust recovery after a timeout mid-transfer: abort, then clear.
4. A vendor-quirks table (a per-VID/PID behavior override map). Start it empty and document each entry with its evidence.
5. **Windows:** detect when the device is bound to the vendor VISA driver instead of WinUSB. Return a clear `BackendUnavailable` explaining the driver situation, and mention Zadig/WinUSB in the docs. On Linux, document the required udev rules (ship an example rules file).

**Exit criteria**
- Conformance and golden-trace replay pass on Linux and macOS against at least two USB instruments from different vendors.
- Windows behavior is documented and verified on at least one instrument with WinUSB.
- Fuzzed header decoder.

**Stop:** review.

### Phase 5: HiSLIP (`TCPIP::host::hislip0::INSTR`)
**Tasks**
1. Synchronous and asynchronous channels (port 4880 by default):
   - `Initialize` / `AsyncInitialize`
   - `Data` / `DataEND` with message IDs
   - `DeviceClear` handshake
   - `AsyncLock`
   - `AsyncStatusQuery`
   - `AsyncRemoteLocalControl`
   - `Trigger`
   - `AsyncServiceRequest`
   - `Error` / `FatalError` handling
   - `AsyncMaximumMessageSize`
2. **Synchronized mode** first. **Overlapped mode** second, behind a capability flag.
3. Later sub-phase: HiSLIP 2.0 encryption/authentication (TLS via `rustls`, SASL). Only do this if the human approves after 5a.

**Exit criteria**
- Conformance and golden-trace replay pass against at least one HiSLIP instrument.
- Fuzzed message decoder.
- Overlapped mode has dedicated tests.

**Stop:** review.

### Phase 6: Serial, discovery, defaults → release 1.0
**Tasks**
1. `rvisa-serial` via the `serialport` crate:
   - `ASRL<n>::INSTR` plus OS path aliases
   - baud, parity, data and stop bits, flow control
   - break
   - end-in and end-out modes (term char / none / last bit)
2. Unified `list()` across all backends: USB enumeration, VXI-11 broadcast, and mDNS (`_lxi._tcp`, `_hislip._tcp`, `_vxi-11._tcp`, `_scpi-raw._tcp`). The result is de-duplicated and records the source backend.
3. Default backend priority becomes pure-Rust first for any transport that passed conformance, with vendor as the fallback.
4. API freeze review, semver audit (`cargo-semver-checks`), docs polish, migration notes for `visa-rs` users.

**Exit criteria**
- All backends pass the conformance suite.
- All traces replay.
- CI is green on all OS and cross targets.
- The human approves the API freeze.

**Stop:** review before the 1.0 publish.

### Phase 7 (optional, after 1.0): Python / pyvisa backend
- A PyO3 + maturin package that exposes the library as a **pyvisa backend** (in the style of pyvisa-py's `@py`), so existing Python test code can use it with `ResourceManager('@rvisa')`.
- Wheels for Linux, Windows, and macOS (x86_64 and arm64).
- This is the main adoption lever. Prioritize it ahead of any optional features from earlier phases.

---

## 9. Agent operating instructions

1. **One phase at a time.** At the end of each phase, stop and report:
   - what was built
   - exit-criteria status (each item: pass, fail, or blocked, with evidence)
   - deviations from this plan
   - open questions
2. **Record decisions.** Any deviation from §5 or §6 goes in `docs/DESIGN.md` with its rationale, *before* you implement it.
3. **Hardware.** You cannot assume access to hardware. Write hardware tests under `#[ignore]` with env-var gating. When a phase needs hardware results, list the exact commands for the human to run and the results you need back.
4. **Never publish** to crates.io or PyPI, and never push tags or create releases. The human does that.
5. **Dependencies:** prefer well-maintained crates (`libloading`, `thiserror`, `nusb`, `serialport`, `tokio` (optional), `tracing`). Every new dependency must pass `cargo-deny` and be justified in `DESIGN.md`.
6. **Logging:** use `tracing` at the transport level, with byte-level I/O at `trace` level. Never log at `info` or higher in library code.
7. **Docs:** every public item gets rustdoc. Every crate gets a README with a minimal example.
8. **When the spec is ambiguous:**
   - Match **NI-VISA's observed behavior** (verified through a golden trace) over your own reading of the spec.
   - Match the IVI specification over pyvisa-py's behavior.
   - Note each such case in `DESIGN.md`.

---

## 10. Toolchain and conventions

- Rust edition 2024. MSRV: latest stable minus 2 releases at Phase 0 start. Pin it in `rust-toolchain.toml` and `Cargo.toml` `rust-version`.
- License: `MIT OR Apache-2.0`.
- Lints: `clippy::pedantic` warn-level in core. `-D warnings` in CI. `#![deny(unsafe_code)]` everywhere except `rvisa-vendor`.
- Error handling: `thiserror` in libraries. No `anyhow` in public APIs. No `unwrap`/`expect` in library code outside tests (enforce with `clippy::unwrap_used`).
- Formatting: default `rustfmt`.
- Commits: Conventional Commits. Keep a `CHANGELOG.md` (Keep a Changelog format).

---

## 11. Reference specifications

Obtain these from the IVI Foundation, IEEE, USB-IF, and the LXI Consortium. Do not commit copies.

| Area | Spec |
|---|---|
| VISA library API | IVI VPP-4.3 (VISA Library), VPP-4.3.2 (C API) |
| Resource naming | IVI VPP-4.3, resource string grammar |
| Message format | IEEE 488.2 (block data, `*IDN?`, status byte) |
| VXI-11 | VXI-11 TCP/IP Instrument Protocol; ONC-RPC (RFC 5531); XDR (RFC 4506) |
| HiSLIP | IVI-6.1 (HiSLIP 1.x / 2.0) |
| USBTMC | USB-IF USBTMC 1.0; USBTMC-USB488 subclass 1.0 |
| LXI discovery | LXI Device Specification (mDNS/DNS-SD service types) |

---

## 12. Hardware test matrix (fill in `docs/HARDWARE.md`)

| Instrument | Interfaces | Vendor VISA traces | socket | vxi11 | usbtmc | hislip |
|---|---|---|---|---|---|---|
| Keithley 3700A | LAN (socket 5025, VXI-11), USB?, GPIB | ☐ | ☐ | ☐ | ☐ | n/a? |
| _add_ | | ☐ | ☐ | ☐ | ☐ | ☐ |
| _add_ | | ☐ | ☐ | ☐ | ☐ | ☐ |

The goal is at least **two vendors per transport** before calling that backend's conformance complete.
