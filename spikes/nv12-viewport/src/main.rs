//! TASK-7 spike: H.264 -> GStreamer -> NV12 -> wgpu -> egui, with a pop-out
//! viewport on a second display.
//!
//! Throwaway code by design (the task is labelled `spike`): the deliverable is
//! the findings doc, not this crate. It is kept in the workspace only so the
//! numbers can be reproduced and so the pure parts — decoder preference, NV12
//! geometry, the newest-wins hand-off — stay covered by `cargo test` while the
//! real implementations (TASK-14, TASK-20, TASK-67) are written against them.
//!
//! ```text
//! spike-nv12-viewport bench [--width W --height H --frames N]
//! spike-nv12-viewport play <file> [--no-pop-out] [--frames N]
//! ```
//!
//! `bench` needs only a wgpu adapter (a software one will do) and measures the
//! upload and conversion path on synthesised 4K frames. `play` additionally
//! needs a display and a GStreamer installation with H.264 decoders.

use std::sync::Arc;

use spike_nv12_viewport::app::{PlayOptions, SpikeApp, native_options};
use spike_nv12_viewport::frames::FrameSlot;
use spike_nv12_viewport::nv12::Nv12Geometry;
use spike_nv12_viewport::{bench, codes, decode};
use sub_core::{SubError, SubResult};

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Measure the upload and conversion path headlessly.
    Bench {
        /// Picture width in pixels.
        width: u32,
        /// Picture height in pixels.
        height: u32,
        /// How many frames to time.
        frames: u32,
    },
    /// Decode a file and show it, with a pop-out viewport.
    Play(PlayArgs),
    /// Decode a file through the real upload path with no window, so the
    /// end-to-end cost is measurable on a machine with no compositor.
    DecodeBench {
        /// The file to decode.
        path: String,
        /// How many frames to time.
        frames: u32,
    },
    /// Synthesise a test clip so `play` has something to decode.
    MakeClip {
        /// Where to write the clip.
        path: String,
        /// Encoder element to use.
        encoder: String,
        /// Picture width in pixels.
        width: u32,
        /// Picture height in pixels.
        height: u32,
        /// How many frames to encode.
        frames: u32,
    },
}

/// Arguments of the `play` subcommand.
#[derive(Debug, PartialEq, Eq)]
struct PlayArgs {
    /// The file to decode.
    path: String,
    /// Whether the second window opens straight away.
    pop_out: bool,
    /// Close after this many painted frames.
    frames: Option<u64>,
}

/// Usage text, printed on a bad command line.
const USAGE: &str = "\
usage:
  spike-nv12-viewport bench [--width W] [--height H] [--frames N]
  spike-nv12-viewport play <file> [--no-pop-out] [--frames N]
  spike-nv12-viewport decode-bench <file> [--frames N]
  spike-nv12-viewport make-clip <file> [--encoder E] [--width W] [--height H] [--frames N]";

/// Build a [`codes::BAD_ARGUMENTS`] error.
fn bad(message: String) -> SubError {
    SubError::new(codes::BAD_ARGUMENTS, message)
}

/// Read a numeric option value.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENTS`] when the value is missing or not a number.
fn number(value: Option<&String>, flag: &str) -> SubResult<u64> {
    value
        .ok_or_else(|| bad(format!("{flag} needs a number")))?
        .parse::<u64>()
        .map_err(|_| bad(format!("{flag} needs a number")))
}

/// Narrow a parsed count to the `u32` the picture sizes use.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENTS`] naming `flag` when the value does not fit.
fn narrow(value: u64, flag: &str) -> SubResult<u32> {
    u32::try_from(value).map_err(|_| bad(format!("{flag} is too large")))
}

/// The file name argument of a subcommand.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENTS`] with the usage text when it is missing.
fn required_path(args: &[String], command: &str) -> SubResult<String> {
    args.get(1)
        .cloned()
        .ok_or_else(|| bad(format!("{command} needs a file\n{USAGE}")))
}

/// Parse `bench`.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENTS`] for an unknown or malformed option.
fn parse_bench(args: &[String]) -> SubResult<Command> {
    let (mut width, mut height, mut frames) = (3840u64, 2160u64, 60u64);
    let mut index = 1;
    while index < args.len() {
        let value = args.get(index + 1);
        match args[index].as_str() {
            "--width" => width = number(value, "--width")?,
            "--height" => height = number(value, "--height")?,
            "--frames" => frames = number(value, "--frames")?,
            other => return Err(bad(format!("unknown option {other}\n{USAGE}"))),
        }
        index += 2;
    }
    Ok(Command::Bench {
        width: narrow(width, "--width")?,
        height: narrow(height, "--height")?,
        frames: narrow(frames, "--frames")?,
    })
}

/// Parse `play`.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENTS`] for a missing file or an unknown option.
fn parse_play(args: &[String]) -> SubResult<Command> {
    let path = required_path(args, "play")?;
    let mut pop_out = true;
    let mut frames = None;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--no-pop-out" => {
                pop_out = false;
                index += 1;
            }
            "--frames" => {
                frames = Some(number(args.get(index + 1), "--frames")?);
                index += 2;
            }
            other => return Err(bad(format!("unknown option {other}\n{USAGE}"))),
        }
    }
    Ok(Command::Play(PlayArgs {
        path,
        pop_out,
        frames,
    }))
}

/// Parse `decode-bench`.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENTS`] for a missing file or an unknown option.
fn parse_decode_bench(args: &[String]) -> SubResult<Command> {
    let path = required_path(args, "decode-bench")?;
    let mut frames = 60u64;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--frames" => frames = number(args.get(index + 1), "--frames")?,
            other => return Err(bad(format!("unknown option {other}\n{USAGE}"))),
        }
        index += 2;
    }
    Ok(Command::DecodeBench {
        path,
        frames: narrow(frames, "--frames")?,
    })
}

/// Parse `make-clip`.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENTS`] for a missing file or an unknown option.
fn parse_make_clip(args: &[String]) -> SubResult<Command> {
    let path = required_path(args, "make-clip")?;
    let (mut width, mut height, mut frames) = (3840u64, 2160u64, 75u64);
    let mut encoder = "vp8enc".to_owned();
    let mut index = 2;
    while index < args.len() {
        let value = args.get(index + 1);
        match args[index].as_str() {
            "--width" => width = number(value, "--width")?,
            "--height" => height = number(value, "--height")?,
            "--frames" => frames = number(value, "--frames")?,
            "--encoder" => {
                encoder
                    .clone_from(value.ok_or_else(|| bad("--encoder needs an element".to_owned()))?);
            }
            other => return Err(bad(format!("unknown option {other}\n{USAGE}"))),
        }
        index += 2;
    }
    Ok(Command::MakeClip {
        path,
        encoder,
        width: narrow(width, "--width")?,
        height: narrow(height, "--height")?,
        frames: narrow(frames, "--frames")?,
    })
}

/// Parse the command line.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENTS`] with the usage text for anything unrecognised.
fn parse(args: &[String]) -> SubResult<Command> {
    match args.first().map(String::as_str) {
        Some("bench") => parse_bench(args),
        Some("play") => parse_play(args),
        Some("decode-bench") => parse_decode_bench(args),
        Some("make-clip") => parse_make_clip(args),
        _ => Err(bad(USAGE.to_owned())),
    }
}

/// Turn a path into the `file://` URI `uridecodebin3` wants.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENTS`] when the path does not exist or cannot be made
/// absolute.
fn uri_for(path: &str) -> SubResult<String> {
    if path.contains("://") {
        return Ok(path.to_owned());
    }
    let absolute = std::path::Path::new(path)
        .canonicalize()
        .map_err(|error| bad(format!("{path}: {error}")))?;
    gstreamer::glib::filename_to_uri(&absolute, None)
        .map(Into::into)
        .map_err(|error| bad(format!("{path}: {error}")))
}

/// Run the headless benchmark and print its report.
fn run_bench(width: u32, height: u32, frames: u32) -> SubResult<()> {
    let geometry = Nv12Geometry::packed(width, height)?;
    let report = bench::run(geometry, frames)?;
    for line in report.lines() {
        println!("{line}");
    }
    Ok(())
}

/// Decode `path` through the upload path with no window and print the report.
fn run_decode_bench(path: &str, frames: u32) -> SubResult<()> {
    let uri = uri_for(path)?;
    // Generous: a software decoder on a cold cache can take seconds to
    // produce its first 4K frame, and an empty report is worse than a wait.
    let report = bench::run_decode(&uri, frames, std::time::Duration::from_mins(1))?;
    for line in report.lines() {
        println!("{line}");
    }
    Ok(())
}

/// Decode `path` and show it, with the pop-out viewport.
fn run_play(args: &PlayArgs) -> SubResult<()> {
    let uri = uri_for(&args.path)?;
    let slot = Arc::new(FrameSlot::new());
    let decoder = decode::Decoder::start(&uri, Arc::clone(&slot))?;
    let options = PlayOptions {
        uri,
        pop_out: args.pop_out,
        max_frames: args.frames,
    };

    eframe::run_native(
        "spike-nv12-viewport",
        native_options(),
        Box::new(move |cc| Ok(Box::new(SpikeApp::new(cc, options, slot, decoder)?))),
    )
    .map_err(|error| SubError::new(codes::WINDOW_FAILED, error.to_string()))
}

fn main() -> std::process::ExitCode {
    let _ = sub_core::logging::init("spike_nv12_viewport=info,sub_media=info");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let outcome = parse(&args).and_then(|command| match command {
        Command::Bench {
            width,
            height,
            frames,
        } => run_bench(width, height, frames),
        Command::Play(args) => run_play(&args),
        Command::DecodeBench { path, frames } => run_decode_bench(&path, frames),
        Command::MakeClip {
            path,
            encoder,
            width,
            height,
            frames,
        } => decode::make_clip(std::path::Path::new(&path), &encoder, width, height, frames),
    });
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[{}] {}", error.code, error.message);
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, PlayArgs, parse};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn bench_defaults_to_sixty_4k_frames() {
        assert_eq!(
            parse(&args(&["bench"])).expect("bench parses"),
            Command::Bench {
                width: 3840,
                height: 2160,
                frames: 60
            }
        );
    }

    #[test]
    fn bench_options_override_the_defaults() {
        assert_eq!(
            parse(&args(&["bench", "--width", "1920", "--frames", "10"]))
                .expect("bench options parse"),
            Command::Bench {
                width: 1920,
                height: 2160,
                frames: 10
            }
        );
    }

    #[test]
    fn play_pops_out_unless_told_not_to() {
        assert_eq!(
            parse(&args(&["play", "clip.mp4"])).expect("play parses"),
            Command::Play(PlayArgs {
                path: "clip.mp4".to_owned(),
                pop_out: true,
                frames: None
            })
        );
        assert_eq!(
            parse(&args(&[
                "play",
                "clip.mp4",
                "--no-pop-out",
                "--frames",
                "5"
            ]))
            .expect("play options parse"),
            Command::Play(PlayArgs {
                path: "clip.mp4".to_owned(),
                pop_out: false,
                frames: Some(5)
            })
        );
    }

    #[test]
    fn an_empty_or_unknown_command_line_is_refused_with_usage() {
        let error = parse(&args(&[])).expect_err("no subcommand is an error");
        assert_eq!(error.code.as_str(), "spike.bad_arguments");
        assert!(error.message.contains("usage"), "{}", error.message);

        let error = parse(&args(&["bench", "--depth", "8"])).expect_err("unknown option");
        assert!(error.message.contains("--depth"), "{}", error.message);

        let error = parse(&args(&["bench", "--frames", "many"])).expect_err("non-numeric count");
        assert!(error.message.contains("--frames"), "{}", error.message);

        let error = parse(&args(&["play"])).expect_err("play needs a file");
        assert!(error.message.contains("needs a file"), "{}", error.message);
    }

    #[test]
    fn decode_bench_defaults_to_sixty_frames() {
        assert_eq!(
            parse(&args(&["decode-bench", "clip.webm"])).expect("decode-bench parses"),
            Command::DecodeBench {
                path: "clip.webm".to_owned(),
                frames: 60
            }
        );
    }

    #[test]
    fn make_clip_defaults_to_a_three_second_4k_vp8_clip() {
        assert_eq!(
            parse(&args(&["make-clip", "clip.webm"])).expect("make-clip parses"),
            Command::MakeClip {
                path: "clip.webm".to_owned(),
                encoder: "vp8enc".to_owned(),
                width: 3840,
                height: 2160,
                frames: 75
            }
        );
    }

    #[test]
    fn make_clip_takes_any_encoder_element() {
        let parsed = parse(&args(&["make-clip", "clip.webm", "--encoder", "x264enc"]))
            .expect("an encoder can be chosen");
        assert!(
            matches!(parsed, Command::MakeClip { ref encoder, .. } if encoder == "x264enc"),
            "{parsed:?}"
        );
    }

    #[test]
    fn a_uri_is_passed_through_untouched() {
        let uri = super::uri_for("https://example.invalid/a.mp4").expect("URIs pass through");
        assert_eq!(uri, "https://example.invalid/a.mp4");
    }

    #[test]
    fn a_missing_file_is_reported_before_any_pipeline_is_built() {
        let error =
            super::uri_for("/definitely/not/here.mp4").expect_err("a missing file is an error");
        assert_eq!(error.code.as_str(), "spike.bad_arguments");
    }
}
