//! The Subordinate GUI binary.
//!
//! Launches the egui application from `sub-ui` together with the engine
//! thread and the local Command API socket.
//!
//! `--smoke-test` (or `SUB_SMOKE_FRAMES=<n>`) starts the window, paints a few
//! empty frames and exits, which is how CI proves the app comes up on each
//! OS without a human at the keyboard.

use sub_ui::AppOptions;

/// Frames painted by `--smoke-test` before the window closes.
const SMOKE_FRAMES: u32 = 3;

fn main() -> std::process::ExitCode {
    // Replaced by the `tracing` setup from TASK-11; until then `RUST_LOG`
    // drives what the startup path prints. The window-system crates are
    // chatty at info level, so they start a notch higher.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,zbus=warn,tracing=warn,calloop=warn"),
    )
    .init();

    let mut options = AppOptions::from_env();
    if std::env::args().skip(1).any(|arg| arg == "--smoke-test") {
        options.smoke_frames = Some(options.smoke_frames.unwrap_or(SMOKE_FRAMES));
    }

    log::info!("subordinate {}", env!("CARGO_PKG_VERSION"));
    if let Err(error) = sub_ui::run(options) {
        log::error!("could not start the editor: {error}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
