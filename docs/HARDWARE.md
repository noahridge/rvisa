# Hardware test matrix

Results of running the conformance suite and recording golden traces on real
instruments. See ROADMAP.md §7.2–§7.3 and §12. The goal is at least two
vendors per transport before a backend's conformance is called complete.

Legend: ☐ not run · ✅ pass · ❌ fail (link the issue) · n/a not applicable.

## Instruments

| Instrument | Firmware | Interfaces | Vendor VISA traces | socket | vxi11 | usbtmc | hislip |
|---|---|---|---|---|---|---|---|
| Keithley 3700A | | LAN (socket 5025, VXI-11), USB?, GPIB | ☐ | ☐ | ☐ | ☐ | n/a? |
| _add_ | | | ☐ | ☐ | ☐ | ☐ | ☐ |
| _add_ | | | ☐ | ☐ | ☐ | ☐ | ☐ |

## Test hosts

| Host OS | VISA implementation and version | Notes |
|---|---|---|
| Windows 11 Pro 10.0.26200, x86_64 | NI-VISA 26.5 (`visa64.dll` 8.0.7331, IVI shared components) | Loads `visa64.dll` from `System32`. Registry has `VXIPNPPATH` only. |

## Phase 1: vendor backend runs

Everything below needs a machine with a VISA installed (NI-VISA, Keysight
IO Libraries or R&S VISA). The exit criteria need Windows **and** Linux;
macOS is best-effort. The library is found automatically, or set
`RVISA_VENDOR_LIB` to its path.

### 1. VISA only, no instrument

Serves the mock demo instrument on a local TCP port and drives it through
VISA as a `SOCKET` resource:

```text
cargo test -p rvisa-vendor --test hardware -- --ignored --nocapture vendor_against_tcp_simulator
```

Send back: the printed `VISA:` path, and the conformance report.

### 2. Constants check

From the VISA install's `visa.h` (look at it; do not commit it, D3),
confirm the values in `crates/rvisa-vendor/src/visa.rs`. In particular,
check the `VI_ERROR_*`/`VI_SUCCESS_*` table, the `VI_ATTR_*` IDs and the
`VI_EVENT_*` IDs. On Windows, also report whether
`HKLM\SOFTWARE\VXIPNP_Alliance\VXIPNP\CurrentVersion` has `VXIPNPPATH64`
and what it points to (DESIGN.md V4).

### 3. Conformance and golden traces, per instrument

```text
# TCPIP (VXI-11 or HiSLIP) instrument
RVISA_HW_VENDOR=TCPIP0::<host>::INSTR \
RVISA_HW_BLOCK_CMD="<query returning a binary block>" \
RVISA_HW_LARGE_CMD="<query returning >= 1 MiB block>" \
RVISA_HW_SRQ_CMD="*CLS;*ESE 1;*SRE 32;*OPC" \
RVISA_HW_TRACE_DIR=traces/<instrument>/tcpip \
  cargo test -p rvisa-vendor --test hardware -- --ignored --nocapture vendor_conformance

# Raw socket: add RVISA_HW_TERM_ALWAYS=1, and allow what sockets cannot do
RVISA_HW_VENDOR=TCPIP0::<host>::5025::SOCKET RVISA_HW_TERM_ALWAYS=1 \
RVISA_HW_ALLOW=stb_and_srq_event,wait_event_requires_enable,assert_trigger \
RVISA_HW_TRACE_DIR=traces/<instrument>/socket \
  cargo test -p rvisa-vendor --test hardware -- --ignored --nocapture vendor_conformance

# USB instrument
RVISA_HW_VENDOR=USB0::<vid>::<pid>::<serial>::INSTR ... (same variables)
```

In PowerShell, set the variables first, then run the same `cargo test`
line:

```text
$env:RVISA_HW_VENDOR = "TCPIP0::<host>::INSTR"
$env:RVISA_HW_SRQ_CMD = "*CLS;*ESE 1;*SRE 32;*OPC"
$env:RVISA_HW_TRACE_DIR = "traces/<instrument>/tcpip"
cargo test -p rvisa-vendor --test hardware -- --ignored --nocapture vendor_conformance
```

Variables persist for the rest of the shell session, so clear them
(`Remove-Item Env:RVISA_HW_*`) before the next instrument.

All variables are documented at the top of
`crates/rvisa-vendor/tests/hardware.rs`. Commands are given without a
terminator. Send back the report for each run, and commit the trace
directories (at least one TCPIP and one USB instrument, per the Phase 1
exit criteria).

## Phase 2: raw socket backend runs

The exit criteria: the socket backend passes conformance, its responses
match the vendor golden traces from the same instrument, and its 1 MB
throughput is within 10% of the vendor backend's. Priority target: the
Keithley 3700A on port 5025.

Record the vendor traces first (Phase 1 step 3, the raw socket command,
into `traces/<instrument>/socket`). Then, with the same instrument and the
same `RVISA_HW_*_CMD` values:

```text
RVISA_HW_SOCKET=TCPIP0::<host>::5025::SOCKET \
RVISA_HW_BLOCK_CMD="<query returning a binary block>" \
RVISA_HW_LARGE_CMD="<query returning >= 1 MiB block>" \
RVISA_HW_GOLDEN_DIR=traces/<instrument>/socket \
  cargo test --release -p rvisa-socket --test hardware -- --ignored --nocapture --test-threads=1
```

This runs three tests: `socket_conformance`, `socket_matches_golden_traces`
and `throughput_1mb`. Use `--release` so the throughput comparison is fair,
and `--test-threads=1` so the tests take turns with the instrument. All
variables are documented at the top of
`crates/rvisa-socket/tests/hardware.rs`. Send back the output, and record
the throughput numbers below.

Without an instrument, the same comparisons run against the mock TCP
instrument, with NI-VISA as the reference:

```text
cargo test --release -p rvisa-socket --test hardware -- --ignored --nocapture --test-threads=1 _on_simulator
```

### NI-VISA SOCKET behaviour (reference for the socket backend)

Observed with NI-VISA 26.5 on Windows 11 on 2026-10-13, against a local
server that logged every byte (DESIGN.md S1–S6):

| Operation | Behaviour |
|---|---|
| Defaults | timeout 2000 ms, term char `\n` disabled, `SEND_END_EN` on, `IO_PROT` normal, `TCPIP_NODELAY` on, `TCPIP_KEEPALIVE` on |
| `viWrite` | sends the bytes unchanged, whatever `TERMCHAR_EN` and `SEND_END_EN` say |
| `viRead` timing out | `VI_ERROR_TMO` with the bytes received so far, which are consumed |
| `viRead` without term char | ends only at the count or a timeout |
| `viClear` | sends nothing; discards input already received, not a reply still in flight |
| `viReadSTB`, `viAssertTrigger`, `IO_PROT` normal | `VI_ERROR_INV_SETUP`, nothing sent |
| `viReadSTB`, `IO_PROT` 488.2 strings | sends `*STB?\n`, reads the reply with the session's term char setting (so it times out with the term char off) |
| `viAssertTrigger`, `IO_PROT` 488.2 strings | sends `*TRG\n` |
| `viEnableEvent` for SRQ | `VI_ERROR_INV_EVENT` |
| `viLock`, `viUnlock` | work; a second `viUnlock` gives `VI_ERROR_SESN_NLOCKED` |

## Run log

One entry per run: date, host, instrument, backend, the command used, and the
outcome (with the conformance report).

| Date | Host | Instrument | Backend | Command | Result |
|---|---|---|---|---|---|
| 2026-10-13 | Windows 11, NI-VISA 26.5 | none (constants check) | vendor | step 2 | ✅ All 165 constants in `visa.rs` and all 22 prototypes in `ffi.rs` match `visa.h`. |
| 2026-10-13 | Windows 11, NI-VISA 26.5 | TCP simulator (`SIM100`) | vendor | step 1 | ✅ 9 passed; `stb_and_srq_event` skipped; `wait_event_requires_enable` and `assert_trigger` unsupported on SOCKET (DESIGN.md V13). The first run failed on those two until V13's mapping was added. |
| 2026-10-13 | Windows 11, NI-VISA 26.5 | local logging server | vendor | `vendor_timeout_keeps_partial_data` | ✅ A timed-out read returns the bytes received (DESIGN.md C12). |
| 2026-10-13 | Windows 11, NI-VISA 26.5 | TCP simulator (`SIM100`) | socket vs vendor | `socket_matches_vendor_on_simulator` | ✅ Both 9 passed, 1 skipped, 2 unsupported. Responses identical for all 12 tests. |

## Throughput (1 MB binary transfer)

Required by the Phase 2 exit criteria. Simulator rows are loopback
indications only; the mock server and loopback dominate them.

| Instrument | Backend | MB/s | Notes |
|---|---|---|---|
| TCP simulator (`SIM100`), loopback | vendor (NI-VISA 26.5) | 75–146 | `--release`, best of 5, four runs |
| TCP simulator (`SIM100`), loopback | socket | 403–615 | same runs |
