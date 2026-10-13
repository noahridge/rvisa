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

## Dependencies

Every dependency passes `cargo-deny` (see `deny.toml`).

| Crate | Used by | Why |
|---|---|---|
| `thiserror` | core | Derives `Error` (required by §10). |
| `regex-lite` | core | VISA search patterns (C8): linear time and no panics, with a smaller footprint than `regex` and no Unicode tables. |
| `serde`, `serde_json` | mock | The JSON Lines trace format (M2). |
| `tracing` | mock | Transport-level logging (§9.6); byte I/O at `trace`. |
| `libfuzzer-sys` | fuzz (not published) | cargo-fuzz targets (§7.4). |
