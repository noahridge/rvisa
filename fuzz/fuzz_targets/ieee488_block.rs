//! The IEEE 488.2 block parsers never panic and stay within the input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use rvisa_core::ieee488::{BlockHeader, parse_block, parse_header};

fuzz_target!(|data: &[u8]| {
    if let Ok(Some(header)) = parse_header(data) {
        let start = match header {
            BlockHeader::Definite { data_start, .. } | BlockHeader::Indefinite { data_start } => {
                data_start
            }
        };
        assert!(start <= data.len());
    }
    if let Ok(block) = parse_block(data) {
        assert!(block.len() <= data.len());
    }
});
