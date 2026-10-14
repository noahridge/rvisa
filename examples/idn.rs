//! Opens a resource and prints its `*IDN?` response.
//!
//! ```text
//! cargo run -p rvisa --example idn -- TCPIP0::192.168.1.50::INSTR
//! cargo run -p rvisa --example idn -- TCPIP0::192.168.1.50::5025::SOCKET
//! ```
//!
//! Socket resources have no END indicator, so this enables newline read
//! termination for them.

use std::time::Duration;

use rvisa::ResourceName;
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
    let Some(resource) = std::env::args().nth(1) else {
        eprintln!("usage: idn RESOURCE");
        std::process::exit(2);
    };
    let rm = ResourceManager::new()?;
    let mut instrument = rm.open(&resource)?;
    instrument.set_timeout(Some(Duration::from_secs(5)))?;
    if matches!(instrument.resource(), ResourceName::TcpipSocket { .. }) {
        instrument.set_read_termination(Some(b'\n'))?;
    }
    let idn = instrument.query("*IDN?")?;
    println!(
        "{} [{}]: {idn}",
        instrument.resource(),
        instrument.backend()
    );
    Ok(())
}
