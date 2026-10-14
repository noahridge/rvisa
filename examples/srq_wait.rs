//! Waits for an instrument to request service (SRQ) when an operation
//! completes, using the event queue.
//!
//! ```text
//! cargo run -p rvisa --example srq_wait -- RESOURCE [OPERATION]
//! cargo run -p rvisa --example srq_wait -- GPIB0::22::INSTR "INIT"
//! ```
//!
//! The instrument is told to assert SRQ on Operation Complete
//! (`*CLS;*ESE 1;*SRE 32`), then `OPERATION` (default `*OPC`, which
//! completes at once) is sent followed by `*OPC`.

use std::time::Duration;

use rvisa::EventKind;
use rvisa::prelude::*;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> rvisa::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(resource) = args.next() else {
        eprintln!("usage: srq_wait RESOURCE [OPERATION]");
        std::process::exit(2);
    };
    let operation = args.next();

    let rm = ResourceManager::new()?;
    let mut instrument = rm.open(&resource)?;
    instrument.clear()?;
    instrument.write_str("*CLS;*ESE 1;*SRE 32")?;
    instrument.enable_event(EventKind::ServiceRequest)?;

    match operation {
        Some(op) => instrument.write_str(&format!("{op};*OPC"))?,
        None => instrument.write_str("*OPC")?,
    }
    let event = instrument.wait_event(EventKind::ServiceRequest, Duration::from_secs(30))?;
    let stb = instrument.read_stb()?;
    println!("{:?}; status byte {stb:#04x}", event.kind);

    instrument.disable_event(EventKind::ServiceRequest)?;
    Ok(())
}
