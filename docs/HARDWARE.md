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

## Run log

One entry per run: date, host, instrument, backend, the command used, and the
outcome (with the conformance report).

| Date | Host | Instrument | Backend | Command | Result |
|---|---|---|---|---|---|

## Throughput (1 MB binary transfer)

Required by the Phase 2 exit criteria.

| Instrument | Backend | MB/s | Notes |
|---|---|---|---|
