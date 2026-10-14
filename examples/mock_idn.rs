//! Lists the mock backend's demo instruments, asks each for its identity,
//! and reads a binary block from one.
//!
//! ```text
//! cargo run -p rvisa --features mock --example mock_idn
//! ```

use rvisa::prelude::*;

fn main() -> rvisa::Result<()> {
    let rm = ResourceManager::new()?;
    println!("backends: {:?}", rm.backend_names());

    for name in rm.list("?*")? {
        let mut instrument = rm.open(&name.to_string())?;
        let idn = instrument.query("*IDN?")?;
        println!("{name} [{}]: {idn}", instrument.backend());
    }

    let mut dmm = rm.open("TCPIP0::mock.local::INSTR")?;
    let data = dmm.query_ieee488_block("DATA?")?;
    let (chunks, _) = data.as_chunks::<4>();
    let values: Vec<f32> = chunks.iter().map(|c| f32::from_le_bytes(*c)).collect();
    println!("DATA? -> {values:?}");
    Ok(())
}
