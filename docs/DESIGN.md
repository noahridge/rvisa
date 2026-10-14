# rvisa design notes

Living record of decisions, and every deviation from [ROADMAP.md](ROADMAP.md)
(§5 API sketch, §6 binding decisions). Each entry says what, why, and when.
Newest entries go at the end of their section.

## Project history

### H1. Earlier prototype parked in `legacy/` (Phase 0)

Before the roadmap existed, the repository held an NI-VISA-specific prototype
(`ni-visa`, `ni-visa-sys`, `ni-visa-mock`). It conflicts with the roadmap:
the crate names use "ni-visa" (trademark); it has no `Session` abstraction;
and it covers non-goals (`viInstallHandler` handlers, register/memory I/O,
PXI/VXI operations, ~130 FFI functions rather than ~40).

It was never committed, so instead of deleting it, it was moved to `legacy/`,
outside the Cargo workspace. `legacy/` is git-ignored, so it stays local and
out of history. Phase 1 may consult it as reference material for
`rvisa-vendor` (loader, status table, a calling convention cross-check). It
must not be built or published, and `legacy/` is deleted once Phase 1 lands.
Its status-code and attribute values were written from the spec without a
header, and are unverified.

## Toolchain

### T1. MSRV 1.97

Phase 0 started on 2026-10-12, when the latest stable was 1.99.0, so per §10
the MSRV is 1.97. It is pinned in `rust-toolchain.toml` and in the workspace
`rust-version`. CI also builds on current stable.

## Core API (`rvisa-core`), deviations from §5

### C1. `ResourceName::AsrlPath` for OS device paths

§5 has `AsrlInstr { board }` and asks for `/dev/ttyUSB0` aliases, which a
board number cannot represent. Added `AsrlPath { path }`, written canonically
as `ASRL/dev/ttyUSB0::INSTR` (the form pyvisa-py uses), so `parse(format(x))
== x` holds. `COM3` parses to `AsrlInstr { board: 3 }`.

### C2. Resource-name normalisation rules

- Interface prefixes and the class suffix are case-insensitive; the
  canonical form is upper case with an explicit board (`TCPIP0::`).
- A missing board means 0. A missing `::INSTR` suffix means `INSTR`.
- Host names, LAN device names (`inst0`, `hislip0`) and USB serial numbers
  keep their case. They are compared case-sensitively by `==`, which keeps
  the round-trip law simple; backends compare device names
  case-insensitively.
- IPv6 hosts must be bracketed (`TCPIP0::[fe80::1]::INSTR`). They are stored
  without brackets and formatted with them.
- Leading and trailing whitespace is ignored; whitespace inside a component
  is an error.
- USB IDs accept `0x` hex or decimal and are formatted `0x%04X`.
- GPIB primary and secondary addresses must be 0–30.
- VISA aliases (`"dmm"`) are not resource names. Resolving them is a
  backend's job (the vendor backend can use `viParseRsrcEx` in Phase 1).

### C3. Write termination: provided trait methods, not backend methods

§5 puts `set_write_termination` on `Session` while D9 says backends never
append it. To honour both, `Session` has *provided* methods
`write_termination()` (default `None`) and `set_write_termination()` (default
`Unsupported`). Backends do not override them. The facade's `Instrument`
overrides both, and `SessionExt::query`/`write_str` append
`write_termination()`. `Session::write` always sends bytes unchanged.

The facade's default write termination is `\n` (the SCPI program message
terminator; pyvisa uses `\r\n`). Read termination defaults to off, as in VISA.

### C4. Additional `Error` variants

`Error` is `#[non_exhaustive]` and §5 ends with `// ...`. Added:
`ResourceLocked`, `AttrReadOnly(Attr)`, `EventNotEnabled(EventKind)`,
`InvalidArgument(String)`, `InvalidData(String)` (malformed instrument data,
such as a bad 488.2 block), and `NoBackend(ResourceName)` (no registered
backend supports the resource).

### C5. Shared locks take a caller-chosen key

`lock(kind, timeout)` returns `()`, so a VISA-generated shared-lock key could
not be returned. `LockKind::Shared { key }` therefore requires the caller to
supply the key, which other sessions pass to join the lock.

### C6. `Event` carries only its kind for now

`Event` is a `#[non_exhaustive]` struct with `kind: EventKind`, plus a
constructor for backends. Event-specific data (such as USB interrupt bytes)
is added when a backend needs it.

### C7. Attribute model

`Attr` is a `#[non_exhaustive]` enum covering the Phase 1 common set plus
`Raw(u32)`. `AttrValue` is a tagged value enum (`Bool`, `U8`, `U16`, `U32`,
`U64`, `I32`, `Str`, `Timeout`, `Parity`, `StopBits`, `FlowControl`,
`InterfaceType`). `Attr::kind()` reports the expected value type so backends
can reject mismatches with `InvalidArgument`. Typed per-attribute accessors
can be layered on top later without breaking this.

### C8. VISA search patterns via `regex-lite`

`Backend::list(pattern)` takes a VISA search expression. VISA's syntax
(`?`, `*`, `+`, `[...]`, `[^...]`, `|`, `(...)`, `\`) is translated to a
`regex-lite` pattern (`?` → `.`, other regex metacharacters escaped), matched
case-insensitively against the whole canonical resource name. `regex-lite`
runs in linear time and rejects bad patterns with an error, so untrusted
patterns cannot panic or hang.

### C9. `SessionExt` additions

Besides §5's four methods, `SessionExt` has `write_all` (repeats `write`
until everything is sent), `write_str` (appends the write termination) and
`read_message` (reads until END or the term char, across `MaxCount`
chunks). `query` strips trailing `\r`/`\n` and decodes lossily.
`query_bytes` sends its bytes unchanged, with no write termination.

### C10. IEEE 488.2 block reading never trusts the length header

The length is used to know when to stop, never to pre-allocate, so a forged
`#9999999999` header cannot exhaust memory. Definite-length blocks keep
reading through `TermChar` stops (binary data may contain the term char) and
then consume the trailing message terminator. Indefinite-length (`#0`)
blocks need an END indicator, so they are not supported on transports
without one (raw sockets).

### C11. `Box<dyn Session>` is a `Session`

`rvisa-core` implements `Session` for `Box<S>`, forwarding every method
(including the provided write-termination methods). Wrappers such as
`RecordingSession<S>` can then wrap the `Box<dyn Session>` that
`Backend::open` returns.

## Mock, record and replay (`rvisa-mock`)

### M1. Scripted instruments

`Script` is built as `Script::new().on_write("*IDN?\n").respond(b"...")`.
Rules match whole writes exactly. Actions are `delay(d)` (the response
becomes readable after `d`), `srq(stb)` (sets the status byte and raises
`ServiceRequest`), `respond(bytes)`, and `timeout()` (the device swallows
the command, so the next read times out). Unmatched writes are recorded and
ignored, like a real instrument, unless `Script::strict()` is set.

Instrument state lives in the backend, per resource, and is shared by every
session opened on it, so output queues and locks behave as they do on a real
device. With an infinite timeout and nothing to read, the mock returns
`Timeout` at once instead of hanging the test.

### M2. Trace format

JSON Lines. The first line is an `open` record (resource and backend name);
every later line is one `Session` call with its arguments, its result
(`{"ok": ...}` or `{"err": {...}}`) and `elapsed_us`. Byte strings are JSON
strings, with printable ASCII kept literally and every other byte (and `\`)
written as `\xNN`, so traces stay readable and diffable. Errors are stored
by kind with their details. `ReplaySession` rebuilds them, using
`"replay"` for backend names, since `&'static str` cannot be deserialised.

`ReplaySession` requires each call to match the next recorded call (operation
and arguments). Otherwise it returns `Error::Protocol` naming the mismatch,
and the recorded call stays in place. Timing is not replayed unless
`ReplaySession::realtime()` is set, which makes each call take as long as it
did when recorded. That is needed to replay code that measures time, such as
the conformance timeout test.

### M3. `RecordingSession<S, W>` owns its writer

The trace writer is a type parameter, and `finish()` returns `(session,
writer)`, so a `Vec<u8>` or a file can be recorded into without `'static`
borrows. Recording failures do not break the session: recording stops, and
`finish()` reports the first error.

### M4. Golden comparison splits messages at END only

`Trace::responses()` joins read data into messages that end at END. Stops
at the term char do not split a message, because they depend on session
settings, not on what the device sent; a binary block containing `\n` would
otherwise come apart differently on different backends. On transports
without END (raw sockets) the responses form one stream.

### M5. Demo instruments

`MockBackend::demo()` registers five instruments (TCPIP INSTR, TCPIP SOCKET,
USB, GPIB, ASRL), all running `demo_script()`: a generic SCPI device whose
commands cover every conformance test. It includes a 1 MiB block containing
every byte value, so term-char handling in binary data is exercised.
In Phase 1 it gained `*CLS;*ESE 1;*SRE 32` and an `*OPC` that raises SRQ
with STB 0x60, so the `srq_wait` example runs end to end against the mock.

## Conformance (`rvisa-conformance`)

### K1. Profiles and explicit outcomes

Instrument commands differ, so each test takes a `Profile` naming the
commands to use (`*IDN?`, a command returning a multi-line response, a binary
block, a ≥ 1 MiB block, one that raises SRQ). Each test returns an `Outcome`:
`Passed`, `Unsupported { op }` (the backend returned `Error::Unsupported`),
`Skipped { reason }` (the profile lacks the needed command) or
`Failed { reason }`. The runner opens a fresh session for each test, and
`Report::assert_all_passed` fails on anything other than `Passed` unless the
test was explicitly allowed. Nothing passes silently.

## Facade (`rvisa`)

### F1. Registry, selection and the mock

`ResourceManager::new()` registers every backend compiled in, in default
priority order. In Phase 0 that is only the `mock` feature's backend, which
comes preloaded with demonstration instruments. `builder()` takes extra
backends (`backend(...)`) and an explicit order (`prefer(&[...])`). Unknown
names in `prefer` are an `InvalidArgument` error. `open` uses the first
backend in priority order whose `supports()` is true, without falling back
on failure. `open_with(name, backend)` forces one. `list(pattern)` merges every
backend's results without duplicates, in priority order.

`open` returns `rvisa::Instrument`, which implements `Session` by delegating
to the backend session, adds write termination (C3), and records which
backend opened it. `into_inner()` returns the backend session.

A builder backend whose name matches one already registered replaces it.
`no_default_backends()` leaves out the compiled-in backends, so tests can
build a manager from exactly the backends they need.

### F2. `rvisa::prelude`

`Session` and `SessionExt` are traits, so calling `set_read_termination` or
`query` on an `Instrument` needs them in scope. `use rvisa::prelude::*`
brings in both, plus `ResourceManager` and `Instrument`, so the §5 facade
example compiles as written.

## Testing

### X1. Fuzzing outside the workspace

The `fuzz/` crate (cargo-fuzz) needs a nightly toolchain, while the
workspace is pinned to the MSRV, so it is its own workspace and is excluded
from the main one. Targets: `resource_name` (no panics, plus the round-trip
law), `ieee488_block` and `search_pattern`. Seed inputs are committed under
`fuzz/seeds/`; generated corpora are not. CI runs each target for 60 s.

### X2. Coverage gate

CI fails if `rvisa-core/src/resource.rs` has less than 95% line coverage
(`cargo llvm-cov`), the Phase 0 exit criterion.

### X3. Miri

Miri runs the unit tests of `rvisa-core`, `rvisa-vendor` and `rvisa-mock`
(ROADMAP Phase 1 exit criterion: the non-FFI parts). Tests that `dlopen` a
library or open sockets are `cfg_attr(miri, ignore)`, since Miri cannot run
them; the FFI itself is covered by the fake-library integration tests.

## Vendor backend (`rvisa-vendor`), Phase 1

### V1. `unsafe` is confined to one module, behind safe wrappers

All FFI lives in `rvisa-vendor/src/ffi.rs`: the type aliases, the function
table, library loading, and one *safe* wrapper method per VISA function.
Each wrapper holds the single `unsafe` call with its `// SAFETY:` comment.
The rest of the crate (session, attributes, status mapping, discovery) is
ordinary safe code, compiled under `#![deny(unsafe_code)]`, with the one
`#[allow(unsafe_code)]` on `mod ffi`. This meets D10's "one module per
concern" with a single concern: crossing the FFI boundary.

### V2. A fake VISA library for CI (deviation from D10)

D10 allows `unsafe` only in `rvisa-vendor`. Testing the real FFI path
without VISA needs a shared library that exports the VISA C functions, and
that needs `unsafe extern` code. `crates/rvisa-vendor-fake` (a `cdylib`,
`publish = false`) is that library: it translates each C call into a call
on an `rvisa-mock` session, so the vendor backend runs the full conformance
suite through real `dlopen` and real pointer marshalling. It is test-only,
never shipped, and every `unsafe` block in it carries a `SAFETY` comment.

### V3. Loading is lazy; `supports()` does not need the library

`VendorBackend::new()` never fails. The library is loaded on first use
(`open`, `list`, `resolve_alias`), and a load failure is cached and reported
as `Error::BackendUnavailable`, listing every path tried and how to fix it
(install a VISA, or set `RVISA_VENDOR_LIB`). `supports()` answers from the
resource type alone (TCPIP, USB, GPIB and ASRL boards; not `AsrlPath`), so
a missing VISA yields that helpful error at `open`, rather than
`NoBackend`. This is how a binary built without VISA "starts, and returns
`BackendUnavailable` cleanly".

`ResourceManager::list` skips backends that are unavailable, as long as at
least one backend answered. If all of them are unavailable, it returns the
first `BackendUnavailable`.

### V4. Library search order

1. A path given to `VendorBackend::with_library_path` or the facade's
   `vendor_library(...)` builder option.
2. The `RVISA_VENDOR_LIB` environment variable.
3. Platform defaults:
   - Windows: `visa64.dll` (64-bit) or `visa32.dll` through the normal DLL
     search path (NI-VISA and the IVI shared components install there). Then
     the VXIplug&play framework directory from the registry
     (`HKLM\SOFTWARE\VXIPNP_Alliance\VXIPNP\CurrentVersion`, value
     `VXIPNPPATH64` or `VXIPNPPATH`), trying `<dir>\Win64\Bin\` and
     `<dir>\WinNT\Bin\`. **Unverified:** the key and layout come from the
     VXIplug&play framework specification and must be checked on a Windows
     host with NI-VISA in Phase 1 task 7.
   - Linux: `libvisa.so` through the dynamic loader, then
     `libvisa.so` / `libvisa.so.*` in `/usr/local/lib`, `/usr/lib64`,
     `/usr/lib`, and `/usr/lib/<arch>-linux-gnu`.
   - macOS: `/Library/Frameworks/VISA.framework/VISA` (NI), then
     `/Library/Frameworks/RsVisa.framework/RsVisa` (R&S).

A library that does not export `viOpenDefaultRM` is rejected, and the next
candidate is tried.

### V5. Status mapping

| VISA status | `Error` |
|---|---|
| `VI_ERROR_TMO` | `Timeout` (but `ResourceLocked` from `viLock`) |
| `VI_ERROR_RSRC_NFOUND` | `NotFound(resource)` |
| `VI_ERROR_RSRC_LOCKED` | `ResourceLocked` |
| `VI_ERROR_NSUP_OPER`, `VI_ERROR_NIMPL_OPER`, `VI_ERROR_NSUP_ATTR`, `VI_ERROR_NSUP_ATTR_STATE`, `VI_ERROR_NSUP_MECH` | `Unsupported { op }` |
| `VI_ERROR_ATTR_READONLY` | `AttrReadOnly(attr)` |
| `VI_ERROR_NENABLED` | `EventNotEnabled(kind)` |
| `VI_ERROR_SESN_NLOCKED` | `InvalidArgument` (unlock without a lock) |
| `VI_ERROR_INV_RSRC_NAME` | `InvalidResource` |
| anything else below zero | `Vendor { status, name, desc }`, with `name` from the static table and `desc` from `viStatusDesc` |

Completion codes: `VI_SUCCESS_TERM_CHAR` gives `TermReason::TermChar`,
`VI_SUCCESS_MAX_CNT` gives `MaxCount`, and any other success gives `End`
(D11). Warnings on other calls are logged at `debug`.

The status-code values were written from the VPP-4.3 specification. Like
those in `legacy/`, they must be checked against a vendor `visa.h` (read,
not committed; D3) in Phase 1 task 7.

### V6. Attributes

The common set maps to fixed `VI_ATTR_*` IDs and C types (`u8`, `u16`,
`u32`, booleans as `ViUInt16`, strings as 256-byte buffers). `Attr::Raw(id)`
reads strings when the ID has the `0xBFFF` prefix, and otherwise reads a
number into a zeroed 8-byte buffer (no VISA attribute is wider), returned
as `AttrValue::U64`. This relies on little-endian byte order, so on
big-endian targets raw numeric reads return `Unsupported`. Raw writes take
`U64`, `U32`, `U16`, `U8` or `Bool`.

`viReadSTB` returns a `ViUInt16`; the status byte is its low 8 bits.

### V7. Locks and open options

`viOpen` is always called with `VI_NO_LOCK`. A lock in `OpenOptions` is
taken afterwards with `viLock`, because `viOpen` cannot pass a shared-lock
key (C5).

### V8. Data received before a read timeout is discarded

On `VI_ERROR_TMO`, `viRead` may report bytes it received. `Session::read`
returns `Error::Timeout`, and those bytes are lost, matching the trait as
specified in §5. Open question for review: should `Error::Timeout` carry
the partial data?

### V9. Alias resolution through backends

`Backend` gains a provided method `resolve_alias(&self, &str) ->
Result<Option<ResourceName>>`, defaulting to `Ok(None)`. The vendor backend
implements it with `viParseRsrcEx`'s expanded name. When a string does not
parse as a resource name, `ResourceManager::open` asks each backend in
priority order. Adding a provided method does not break backends.

### V10. Priority with both mock and vendor enabled

The `mock` backend, if compiled in, comes *before* `vendor`. It supports only
the resources registered with it, so it cannot capture real instruments,
while vendor supports every VISA resource type and would otherwise capture
the mock's demo resources.

### V11. Async adapter (`tokio` feature)

`rvisa::tokio::AsyncInstrument` wraps an `Instrument` in
`Arc<Mutex<...>>`, and runs each call with `tokio::task::spawn_blocking`
(D8). If a future is dropped, the blocking call still completes and
releases the lock, so the instrument is never lost or left half-used.
`AsyncResourceManager::open` also runs on the blocking pool, since opening
can block (TCP connect, lock wait). A failed join (a panic inside the
backend) is reported as `Error::Io`.

### V12. Vendor-only operations

`viFlush` and the trivial GPIB control call (`viGpibControlREN`) have no
place on the transport-agnostic `Session` trait. They are inherent methods
on `VendorSession`, reachable through `VendorBackend::open_vendor`.

## Conformance additions (Phase 1)

### K2. Transports without END

Raw sockets have no END indicator, so a read without a term char only ends
at a timeout. `Profile::term_char_always` makes every test enable the term
char, as a socket user must. `Report::assert_conformant(names)` now accepts
`Skipped` as well as `Unsupported` for the named tests, so a profile without
an SRQ command can still be asserted on.

## Mock additions (Phase 1)

### M6. A mock instrument on a real TCP socket

`rvisa_mock::TcpInstrument` serves a `Script` on a local TCP port, with raw
socket semantics (newline-terminated commands, no END). With a real VISA
installed, `TCPIP0::127.0.0.1::<port>::SOCKET` then exercises the vendor
backend end to end without an instrument. It is also the Phase 2 socket
backend's first test peer.

## Dependencies

Every dependency passes `cargo-deny` (see `deny.toml`).

| Crate | Used by | Why |
|---|---|---|
| `thiserror` | core | Derives `Error` (required by §10). |
| `regex-lite` | core | VISA search patterns (C8): linear time and no panics, with a smaller footprint than `regex` and no Unicode tables. |
| `serde`, `serde_json` | mock | The JSON Lines trace format (M2). |
| `tracing` | mock | Transport-level logging (§9.6); byte I/O at `trace`. |
| `libfuzzer-sys` | fuzz (not published) | cargo-fuzz targets (§7.4). |
| `libloading` | vendor | Runtime loading of the vendor library (D1). Its licence is ISC (permissive, MIT-equivalent), added to `deny.toml`. |
| `windows-registry` | vendor (Windows only) | Registry lookup of the VISA framework directory (D2, V4). Maintained by Microsoft as part of windows-rs; no `unsafe` needed on our side. |
| `tokio` (optional, `rt`) | facade, feature `tokio` | `spawn_blocking` async adapter (D8, V11). |
