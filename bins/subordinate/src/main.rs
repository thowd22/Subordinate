//! The Subordinate GUI binary.
//!
//! Launches the egui application from `sub-ui` together with the engine
//! thread and the local Command API socket.
//!
//! Two flags exist for CI, and neither needs a human at the keyboard:
//!
//! - `--smoke-test` (or `SUB_SMOKE_FRAMES=<n>`) starts the window, paints a
//!   few empty frames and exits, which is how CI proves the app comes up on
//!   each OS.
//! - `--ui-smoke` opens a project, pops the viewer out, holds both windows on
//!   screen and prints a ready line, which is what `scripts/ui-smoke.sh`
//!   photographs under Xvfb (TASK-123).

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use sub_ui::AppOptions;

/// Frames painted by `--smoke-test` before the window closes.
const SMOKE_FRAMES: u32 = 3;

/// How long `--ui-smoke` leaves the windows on screen by default.
///
/// Long enough for a capture step to run twice over, short enough that a
/// forgotten run cannot hold a CI job open.
const UI_SMOKE_HOLD: Duration = Duration::from_secs(20);

/// Default log filter. The window-system crates are chatty at info level, so
/// they start a notch higher.
const DEFAULT_FILTER: &str = "info,zbus=warn,tracing=warn,calloop=warn";

/// What the command line asked for.
const USAGE: &str = "\
Usage: subordinate [OPTIONS] [PROJECT]

  PROJECT                  a .sub project file to open at startup

  --smoke-test             paint a few frames and exit (CI)
  --ui-smoke               open the pop-out viewer, hold the windows on
                           screen and print a ready line (CI)
  --popout-position X,Y    place the pop-out window there, in points
  --hold-seconds N         close the windows after N seconds
  -h, --help               show this help";

fn main() -> ExitCode {
    if let Err(err) = sub_core::logging::init(DEFAULT_FILTER) {
        eprintln!("{err}");
        return ExitCode::FAILURE;
    }

    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let options = match parse_args(&arguments, AppOptions::from_env()) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("subordinate: {problem}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "subordinate starting");
    if let Err(error) = sub_ui::run(options) {
        tracing::error!("could not start the editor: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Applies `arguments` to the options the environment already supplied.
///
/// # Errors
///
/// A message for the user when an option is unknown, is missing its value or
/// cannot be parsed, or when more than one project is named.
fn parse_args(arguments: &[String], mut options: AppOptions) -> Result<AppOptions, String> {
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--smoke-test" => {
                options.smoke_frames = Some(options.smoke_frames.unwrap_or(SMOKE_FRAMES));
            }
            "--ui-smoke" => {
                options.open_popout = true;
                options.hold = Some(options.hold.unwrap_or(UI_SMOKE_HOLD));
            }
            "--popout-position" => {
                let value = rest
                    .next()
                    .ok_or_else(|| "--popout-position needs X,Y".to_owned())?;
                options.popout_position = Some(parse_position(value)?);
            }
            "--hold-seconds" => {
                let value = rest
                    .next()
                    .ok_or_else(|| "--hold-seconds needs a number".to_owned())?;
                let seconds: f32 = value
                    .trim()
                    .parse()
                    .map_err(|_| format!("--hold-seconds wants a number, not {value:?}"))?;
                if !seconds.is_finite() || seconds <= 0.0 {
                    return Err(format!(
                        "--hold-seconds wants a positive number, not {value}"
                    ));
                }
                options.hold = Some(Duration::from_secs_f32(seconds));
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}"));
            }
            path => {
                if options.project.is_some() {
                    return Err("only one project can be opened".to_owned());
                }
                options.project = Some(PathBuf::from(path));
            }
        }
    }
    Ok(options)
}

/// Parses an `X,Y` window position in points.
fn parse_position(value: &str) -> Result<[f32; 2], String> {
    let (x, y) = value
        .split_once(',')
        .ok_or_else(|| format!("a window position is X,Y, not {value:?}"))?;
    let parse = |part: &str, axis: char| -> Result<f32, String> {
        part.trim()
            .parse::<f32>()
            .ok()
            .filter(|number| number.is_finite())
            .ok_or_else(|| format!("{axis} in {value:?} is not a number"))
    };
    Ok([parse(x, 'x')?, parse(y, 'y')?])
}

#[cfg(test)]
mod tests {
    use super::{SMOKE_FRAMES, UI_SMOKE_HOLD, parse_args, parse_position};
    use std::path::Path;
    use std::time::Duration;
    use sub_ui::AppOptions;

    fn parse(arguments: &[&str]) -> Result<AppOptions, String> {
        let owned: Vec<String> = arguments.iter().map(|arg| (*arg).to_owned()).collect();
        parse_args(&owned, AppOptions::default())
    }

    #[test]
    fn no_arguments_run_normally() {
        let options = parse(&[]).expect("an empty command line is valid");
        assert_eq!(options.smoke_frames, None);
        assert_eq!(options.hold, None);
        assert!(!options.open_popout);
        assert_eq!(options.project, None);
        assert!(!options.is_unattended());
    }

    #[test]
    fn smoke_test_paints_a_few_frames() {
        let options = parse(&["--smoke-test"]).expect("--smoke-test is valid");
        assert_eq!(options.smoke_frames, Some(SMOKE_FRAMES));
        assert!(options.is_unattended());
    }

    #[test]
    fn an_environment_frame_count_survives_the_flag() {
        let from_env = AppOptions {
            smoke_frames: Some(42),
            ..AppOptions::default()
        };
        let options = parse_args(&["--smoke-test".to_owned()], from_env)
            .expect("SUB_SMOKE_FRAMES and --smoke-test agree");
        assert_eq!(options.smoke_frames, Some(42));
    }

    #[test]
    fn ui_smoke_opens_the_popout_and_holds_the_window() {
        let options = parse(&["--ui-smoke"]).expect("--ui-smoke is valid");
        assert!(options.open_popout);
        assert_eq!(options.hold, Some(UI_SMOKE_HOLD));
        assert!(options.is_unattended());
    }

    #[test]
    fn the_hold_can_be_shortened_or_lengthened() {
        let options = parse(&["--ui-smoke", "--hold-seconds", "2.5"]).expect("a hold is valid");
        assert_eq!(options.hold, Some(Duration::from_secs_f32(2.5)));
    }

    #[test]
    fn a_project_is_positional() {
        let options = parse(&["--ui-smoke", "project.sub"]).expect("a project is valid");
        assert_eq!(options.project.as_deref(), Some(Path::new("project.sub")));
    }

    #[test]
    fn the_popout_can_be_placed_on_another_monitor() {
        let options = parse(&["--popout-position", "1280,0"]).expect("a window position is valid");
        assert_eq!(options.popout_position, Some([1280.0, 0.0]));
    }

    #[test]
    fn positions_are_parsed_from_a_pair() {
        assert_eq!(parse_position(" 12 , -8 "), Ok([12.0, -8.0]));
        assert!(parse_position("1280").is_err());
        assert!(parse_position("left,0").is_err());
        assert!(parse_position("nan,0").is_err());
    }

    /// The CI script drives this binary by flag name, so a rename here has to
    /// be a rename there too (TASK-123).
    #[test]
    fn the_ci_script_and_this_binary_agree_on_the_flags() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui-smoke.sh");
        let text = std::fs::read_to_string(&script)
            .unwrap_or_else(|err| panic!("{} should be readable: {err}", script.display()));
        for flag in [
            "--ui-smoke",
            "--hold-seconds",
            "--popout-position",
            sub_ui::UI_SMOKE_READY,
        ] {
            assert!(
                text.contains(flag),
                "scripts/ui-smoke.sh should still use {flag}"
            );
        }
    }

    #[test]
    fn bad_command_lines_are_refused() {
        assert!(parse(&["--nope"]).is_err());
        assert!(parse(&["--popout-position"]).is_err());
        assert!(parse(&["--hold-seconds", "soon"]).is_err());
        assert!(parse(&["--hold-seconds", "0"]).is_err());
        assert!(parse(&["one.sub", "two.sub"]).is_err());
    }
}
