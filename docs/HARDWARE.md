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
| _add_ | | |

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
RVISA_HW_ALLOW=stb_and_srq_event RVISA_HW_TRACE_DIR=traces/<instrument>/socket \
  cargo test -p rvisa-vendor --test hardware -- --ignored --nocapture vendor_conformance

# USB instrument
RVISA_HW_VENDOR=USB0::<vid>::<pid>::<serial>::INSTR ... (same variables)
```

All variables are documented at the top of
`crates/rvisa-vendor/tests/hardware.rs`. Commands are given without a
terminator. Send back the report for each run, and commit the trace
directories (at least one TCPIP and one USB instrument, per the Phase 1
exit criteria).

## Run log

One entry per run: date, host, instrument, backend, the command used, and the
outcome (with the conformance report).

| Date | Host | Instrument | Backend | Command | Result |
|---|---|---|---|---|---|

## Throughput (1 MB binary transfer)

Required by the Phase 2 exit criteria.

| Instrument | Backend | MB/s | Notes |
|---|---|---|---|
