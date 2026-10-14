# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `rvisa-core`: `ResourceName` parser and formatter, the `Session`,
  `SessionExt` and `Backend` traits, `Error`, attributes, events, VISA
  search patterns and an IEEE 488.2 block reader.
- `rvisa-mock`: scripted mock backend, `RecordingSession` and
  `ReplaySession` with a JSON Lines trace format.
- `rvisa-conformance`: backend-agnostic conformance suite.
- `rvisa`: `ResourceManager` with a backend registry and priority selection;
  mock backend behind the `mock` feature.
- `rvisa-vendor`: the vendor VISA backend (NI, Keysight, R&S), loaded at
  runtime. It covers message I/O, typed attributes plus `Attr::Raw`, locks,
  the SRQ event queue, `viFindRsrc` discovery, alias resolution through
  `viParseRsrcEx`, `viFlush` and GPIB REN control. Without VISA, operations
  return `BackendUnavailable` with instructions.
- `rvisa`: `vendor` feature (default), `vendor_library(...)` builder option,
  alias fallback in `open`, and a `list` that skips unavailable backends.
- `rvisa`: `tokio` feature with `AsyncResourceManager` and `AsyncInstrument`
  (`spawn_blocking` adapter).
- `rvisa-core`: `Backend::resolve_alias` (provided method).
- `rvisa-conformance`: `Profile::term_char_always` for transports without END.
- `rvisa-mock`: `TcpInstrument`, a scripted instrument on a local TCP socket.
- Examples: `list_resources`, `idn`, `binary_block_capture`, `srq_wait`.
- `rvisa-socket`: pure-Rust raw TCP socket backend for
  `TCPIP::host::port::SOCKET`, matching NI-VISA's observed behaviour, with
  488.2 string emulation for `read_stb`/`assert_trigger`, a clear command,
  in-process locks, and a native async session (feature `tokio`).
- `rvisa`: `socket` feature (default), registered after `vendor`.
- `rvisa-core`: `LockTable`, VISA lock bookkeeping for backends without
  protocol locks; `Error::timeout`, `Error::read_timeout`,
  `Error::is_timeout` and `Error::prepend_partial`.

### Changed

- **Breaking:** `Error::Timeout` is now `Timeout { partial }`, carrying the
  bytes a read received before timing out, as NI-VISA reports them.
  `SessionExt::read_message` and `read_ieee488_block` include what they
  had read so far. Match it with `Error::Timeout { .. }`.
- `rvisa`: `ResourceManager::open` skips a backend that returns
  `BackendUnavailable` and tries the next one that supports the resource.
- `rvisa-mock`: traces record a timeout's partial data; older traces still
  load. `Trace::responses()` includes it.

### Fixed

- `rvisa-vendor`: NI-VISA's `VI_ERROR_INV_EVENT` on event calls and
  `VI_ERROR_INV_SETUP` on `viAssertTrigger` now map to `Unsupported`.
