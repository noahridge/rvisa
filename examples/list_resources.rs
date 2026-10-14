//! Lists every resource the registered backends can find.
//!
//! ```text
//! cargo run -p rvisa --example list_resources [PATTERN]
//! ```
//!
//! `PATTERN` is a VISA search expression (default `?*INSTR`). Without a
//! VISA installed this prints why the vendor backend is unavailable.

use rvisa::ResourceManager;

fn main() -> std::process::ExitCode {
    let pattern = std::env::args().nth(1).unwrap_or_else(|| "?*INSTR".into());
    let rm = match ResourceManager::new() {
        Ok(rm) => rm,
        Err(e) => return fail(&e),
    };
    println!("backends: {:?}", rm.backend_names());
    match rm.list(&pattern) {
        Ok(names) if names.is_empty() => println!("no resources match {pattern:?}"),
        Ok(names) => names.iter().for_each(|name| println!("{name}")),
        Err(e) => return fail(&e),
    }
    std::process::ExitCode::SUCCESS
}

fn fail(e: &rvisa::Error) -> std::process::ExitCode {
    eprintln!("error: {e}");
    std::process::ExitCode::FAILURE
}
