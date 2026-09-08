//! Headless command-line interface.
//!
//! Loads, saves, inspects and renders projects without a GUI, serves the
//! Command API for out-of-process clients, and scaffolds and tests plugins.
//! It is also the CI smoke test.

fn main() {
    println!("subordinate-cli {}", env!("CARGO_PKG_VERSION"));
}
