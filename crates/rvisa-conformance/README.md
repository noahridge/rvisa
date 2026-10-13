# rvisa-conformance

A backend-agnostic conformance suite. Every `rvisa` backend runs it against
the mock in CI, and against real instruments in tests gated by environment
variables.

```rust,ignore
use rvisa_conformance::{Profile, run_all};

let profile = Profile::scpi().binary_block_command(b"CURV?\n", None);
let report = run_all(|| backend.open(&resource, &OpenOptions::new()), &profile);
report.assert_all_passed();
```

Tests return `Passed`, `Unsupported`, `Skipped` or `Failed`. Nothing passes
silently.
