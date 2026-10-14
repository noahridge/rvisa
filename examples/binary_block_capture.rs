//! Sends a query that returns an IEEE 488.2 binary block (such as a
//! waveform) and saves the block's data to a file.
//!
//! ```text
//! cargo run -p rvisa --example binary_block_capture -- RESOURCE [COMMAND] [OUTPUT]
//! cargo run -p rvisa --example binary_block_capture -- USB0::0x0699::0x0368::C012345::INSTR CURV? curve.bin
//! ```
//!
//! `COMMAND` defaults to `CURV?`; `OUTPUT` to `block.bin`.

use std::time::{Duration, Instant};

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
        eprintln!("usage: binary_block_capture RESOURCE [COMMAND] [OUTPUT]");
        std::process::exit(2);
    };
    let command = args.next().unwrap_or_else(|| "CURV?".into());
    let output = args.next().unwrap_or_else(|| "block.bin".into());

    let rm = ResourceManager::new()?;
    let mut instrument = rm.open(&resource)?;
    instrument.set_timeout(Some(Duration::from_secs(10)))?;

    let start = Instant::now();
    let data = instrument.query_ieee488_block(&command)?;
    let elapsed = start.elapsed();
    std::fs::write(&output, &data)?;
    #[allow(clippy::cast_precision_loss)]
    let rate = data.len() as f64 / elapsed.as_secs_f64() / 1e6;
    println!(
        "{} bytes in {elapsed:.2?} ({rate:.2} MB/s), saved to {output}",
        data.len()
    );
    Ok(())
}
