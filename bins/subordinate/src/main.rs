//! The Subordinate GUI binary.
//!
//! Launches the egui application from `sub-ui` together with the engine
//! thread and the local Command API socket.

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(err) = sub_core::logging::init("info") {
        eprintln!("{err}");
        return ExitCode::FAILURE;
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "subordinate starting");
    println!("subordinate {}", env!("CARGO_PKG_VERSION"));
    ExitCode::SUCCESS
}
