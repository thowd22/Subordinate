//! The Subordinate GUI binary.
//!
//! Launches the egui application from `sub-ui` together with the engine
//! thread and the local Command API socket.

fn main() {
    println!("subordinate {}", env!("CARGO_PKG_VERSION"));
}
