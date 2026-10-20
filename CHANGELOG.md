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
- `rvisa-core`: test cases ported from pyvisa's suite (resource names,
  search expressions, IEEE 488.2 blocks).

### Fixed

- `rvisa-core`: `ieee488::encode_header` now returns `Result`. It used to write
  a corrupt header for blocks of 1 GB or more.
- `rvisa-vendor`: NI-VISA's `VI_ERROR_INV_EVENT` on event calls and
  `VI_ERROR_INV_SETUP` on `viAssertTrigger` now map to `Unsupported`.
