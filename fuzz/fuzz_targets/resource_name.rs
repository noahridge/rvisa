//! Resource parsing never panics, and every name it accepts round-trips
//! through its canonical form.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rvisa_core::ResourceName;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(name) = text.parse::<ResourceName>() {
        let canonical = name.to_string();
        let reparsed: ResourceName = canonical
            .parse()
            .unwrap_or_else(|e| panic!("canonical form {canonical:?} of {text:?} does not parse: {e}"));
        assert_eq!(reparsed, name, "round trip of {text:?} via {canonical:?}");
        assert_eq!(reparsed.to_string(), canonical);
    }
});
