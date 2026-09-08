//! The Subordinate GUI binary.
//!
//! Launches the egui application from `sub-ui` together with the engine
//! thread and the local Command API socket.
//!
//! `--smoke-test` (or `SUB_SMOKE_FRAMES=<n>`) starts the window, paints a few
//! empty frames and exits, which is how CI proves the app comes up on each
//! OS without a human at the keyboard.

use std::process::ExitCode;

use sub_ui::AppOptions;

/// Frames painted by `--smoke-test` before the window closes.
const SMOKE_FRAMES: u32 = 3;

/// Default log filter. The window-system crates are chatty at info level, so
/// they start a notch higher.
const DEFAULT_FILTER: &str = "info,zbus=warn,tracing=warn,calloop=warn";

fn main() -> ExitCode {
    if let Err(err) = sub_core::logging::init(DEFAULT_FILTER) {
        eprintln!("{err}");
        return ExitCode::FAILURE;
    }

    let mut options = AppOptions::from_env();
    if std::env::args().skip(1).any(|arg| arg == "--smoke-test") {
        options.smoke_frames = Some(options.smoke_frames.unwrap_or(SMOKE_FRAMES));
    }

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "subordinate starting");
    if let Err(error) = sub_ui::run(options) {
        tracing::error!("could not start the editor: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
