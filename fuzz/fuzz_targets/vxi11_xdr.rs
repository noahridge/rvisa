//! The VXI-11 XDR and RPC decoders never panic, whatever the device sends.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    rvisa_vxi11::fuzzing::decode_all(data);
});
