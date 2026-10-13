//! Untrusted VISA search expressions compile or fail cleanly, and matching
//! never panics.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rvisa_core::Pattern;

fuzz_target!(|input: (&str, &str)| {
    let (expr, name) = input;
    if let Ok(pattern) = Pattern::new(expr) {
        let _ = pattern.matches_str(name);
    }
});
