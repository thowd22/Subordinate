//! Headless command-line interface.
//!
//! Loads, saves, inspects and renders projects without a GUI, serves the
//! Command API for out-of-process clients, and scaffolds and tests plugins.
//! It is also the CI smoke test.

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(err) = sub_core::logging::init("info") {
        eprintln!("{err}");
        return ExitCode::FAILURE;
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "subordinate-cli starting"
    );
    println!("subordinate-cli {}", env!("CARGO_PKG_VERSION"));
    ExitCode::SUCCESS
}
