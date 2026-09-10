//! Benchmark harness for scrub and playback performance.
//!
//! Measures two things on the 1080p and 4K colour-bar fixtures, through the
//! production decode and upload paths:
//!
//! * **decode-to-texture latency** — decoder hand-off, both NV12 plane
//!   uploads and the YUV-to-RGB pass, waited to GPU completion;
//! * **sustained fps** — frames per second across a whole run, both for
//!   sequential playback and for a scrub that seeks across the file.
//!
//! Results are written as JSON (`--out`) and summarised on stdout, which is
//! what the CI log shows. Baselines live in `docs/PERFORMANCE.md`.
//!
//! `--sync` runs a different measurement instead: the A/V sync and drift
//! harness of [`sync`], which plays the ten-minute long fixture headlessly and
//! reports how far the picture ever gets from the audio clock. It is the only
//! mode that can fail a build on a number rather than on an environment
//! problem: drift of a whole frame or more exits non-zero.
//!
//! ```text
//! subordinate-bench [--out PATH] [--frames N] [--seeks N] [--warmup N]
//!                   [--fixtures DIR] [--software] [--no-gpu]
//! subordinate-bench --sync [--out PATH] [--seconds N] [--fixtures DIR]
//!                   [--software]
//! ```
//!
//! The harness never fails a build because of the machine it runs on: a
//! missing fixture is a skipped scenario and a machine with no wgpu adapter
//! measures decode alone, both recorded in the report.

use std::path::PathBuf;
use std::process::ExitCode;

use sub_core::{ResultExt as _, SubError, SubResult};
use sub_media::HardwarePreference;

mod report;
mod run;
mod stats;
mod sync;

/// Stable [`sub_core::ErrorCode`] constants this binary returns.
///
/// Codes are part of the contract with whatever reads the harness's output:
/// an existing one is never renamed or given a new meaning.
pub mod codes {
    use sub_core::ErrorCode;

    /// An argument was missing, malformed, or not a positive count.
    pub const BAD_ARGUMENT: ErrorCode = ErrorCode::from_static("bench.bad_argument");
    /// The report could not be written to the requested path.
    pub const REPORT_UNWRITABLE: ErrorCode = ErrorCode::from_static("bench.report_unwritable");
    /// A decoded frame is not a usable NV12 picture, so it cannot go through
    /// the compositor's upload path.
    pub const FRAME_LAYOUT: ErrorCode = ErrorCode::from_static("bench.frame_layout");
    /// The GPU stopped responding while a frame was in flight.
    pub const GPU_LOST: ErrorCode = ErrorCode::from_static("bench.gpu_lost");
    /// The picture drifted a whole frame or more from the audio clock, which
    /// is phase 3's exit criterion failing.
    pub const DRIFT_EXCEEDED: ErrorCode = ErrorCode::from_static("bench.drift_exceeded");
}

/// Where the report goes when `--out` is not given.
const DEFAULT_OUT: &str = "target/bench/perf.json";

/// Where the sync report goes when `--out` is not given.
const DEFAULT_SYNC_OUT: &str = "target/bench/av-sync.json";

/// The usage text, printed by `--help` and by a bad argument.
const USAGE: &str = "\
Usage: subordinate-bench [options]

Measures decode-to-texture latency and sustained playback and scrub frame
rates on the 1080p and 4K media fixtures.

Options:
  --out PATH        write the JSON report here (default: target/bench/perf.json)
  --frames N        frames timed per playback scenario (default: 60)
  --seeks N         seeks timed per scrub scenario (default: 30)
  --warmup N        frames decoded before timing starts (default: 3)
  --fixtures DIR    read the fixtures from DIR instead of the usual location
  --software        keep hardware decoders out of the measurement
  --no-gpu          measure decode alone, with no texture upload
  --sync            measure A/V sync and drift over the long fixture instead
                    (default report: target/bench/av-sync.json); exits
                    non-zero when the picture drifts a whole frame or more
  --seconds N       with --sync, stop after N seconds of timeline
  -h, --help        print this text
";

/// What the arguments asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// Print the usage text.
    Help,
    /// Run the harness and write the report to this path.
    Measure {
        /// Where the JSON report goes.
        out: PathBuf,
        /// How the run is configured.
        options: run::Options,
    },
    /// Measure A/V sync and drift instead, writing the report to this path.
    Sync {
        /// Where the JSON report goes.
        out: PathBuf,
        /// How the run is configured.
        options: sync::Options,
    },
}

/// Parses the command line.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENT`] for an unknown option, a missing value, or a count
/// that is not a positive integer.
fn parse(args: &[String]) -> SubResult<Command> {
    let mut out: Option<PathBuf> = None;
    let mut options = run::Options::default();
    let mut run_sync = false;
    let mut seconds: Option<u64> = None;
    let mut index = 0;

    while index < args.len() {
        let arg = args[index].as_str();
        index += 1;
        match arg {
            "-h" | "--help" => return Ok(Command::Help),
            "--software" => options.hardware = HardwarePreference::Software,
            "--no-gpu" => options.use_gpu = false,
            "--sync" => run_sync = true,
            "--seconds" => seconds = Some(count(args, &mut index, arg)?),
            "--out" => out = Some(PathBuf::from(value(args, &mut index, arg)?)),
            "--fixtures" => {
                options.fixtures_dir = Some(PathBuf::from(value(args, &mut index, arg)?));
            }
            "--frames" => options.frames = count(args, &mut index, arg)?,
            "--seeks" => options.seeks = count(args, &mut index, arg)?,
            "--warmup" => {
                let raw = value(args, &mut index, arg)?;
                options.warmup = raw
                    .parse::<u64>()
                    .sub_context_with(codes::BAD_ARGUMENT, || {
                        format!("{arg} needs a whole number of frames, not '{raw}'")
                    })?;
            }
            other => {
                return Err(SubError::new(
                    codes::BAD_ARGUMENT,
                    format!("unknown option '{other}'"),
                ));
            }
        }
    }
    if run_sync {
        return Ok(Command::Sync {
            out: out.unwrap_or_else(|| PathBuf::from(DEFAULT_SYNC_OUT)),
            options: sync::Options {
                fixtures_dir: options.fixtures_dir,
                hardware: options.hardware,
                seconds,
            },
        });
    }
    if seconds.is_some() {
        return Err(SubError::new(
            codes::BAD_ARGUMENT,
            "--seconds only applies to --sync",
        ));
    }
    Ok(Command::Measure {
        out: out.unwrap_or_else(|| PathBuf::from(DEFAULT_OUT)),
        options,
    })
}

/// The value that follows an option, advancing past it.
fn value(args: &[String], index: &mut usize, option: &str) -> SubResult<String> {
    let found = args
        .get(*index)
        .ok_or_else(|| SubError::new(codes::BAD_ARGUMENT, format!("{option} needs a value")))?;
    *index += 1;
    Ok(found.clone())
}

/// A count option, which must be a positive whole number.
fn count(args: &[String], index: &mut usize, option: &str) -> SubResult<u64> {
    let raw = value(args, index, option)?;
    let parsed = raw
        .parse::<u64>()
        .sub_context_with(codes::BAD_ARGUMENT, || {
            format!("{option} needs a whole number, not '{raw}'")
        })?;
    if parsed == 0 {
        return Err(SubError::new(
            codes::BAD_ARGUMENT,
            format!("{option} must be at least 1"),
        ));
    }
    Ok(parsed)
}

/// Runs the harness, writes the report and prints the summary.
///
/// # Errors
///
/// Whatever the measurement reports, plus [`codes::REPORT_UNWRITABLE`] when
/// the JSON cannot be written.
fn measure(out: &std::path::Path, options: &run::Options) -> SubResult<()> {
    let report = run::run(options)?;
    write_report(out, &report)?;

    for line in report.summary_lines() {
        println!("{line}");
    }
    println!("report       {}", out.display());
    if !report.measured_anything() {
        println!("nothing was measured: generate the fixtures with scripts/gen-fixtures.sh first");
    }
    Ok(())
}

/// Runs the A/V sync harness, writes the report and prints the summary.
///
/// # Errors
///
/// [`codes::DRIFT_EXCEEDED`] when the picture drifted a whole frame or more
/// from the audio clock — the number itself failing, which is the point of
/// the harness — plus whatever the measurement or the report writer reports.
/// A machine without the long fixture is a skipped section, not an error.
fn measure_sync(out: &std::path::Path, options: &sync::Options) -> SubResult<()> {
    let section = sync::run(options)?;
    let mut report = report::Report::new(None);
    report.sync = Some(section.clone());
    write_report(out, &report)?;

    for line in report.summary_lines() {
        println!("{line}");
    }
    println!("report       {}", out.display());
    if section.status == report::ScenarioStatus::Skipped {
        println!(
            "A/V sync was not measured: generate the long fixture with \
             scripts/gen-fixtures.sh --long first"
        );
        return Ok(());
    }
    if !section.within_one_frame() {
        return Err(SubError::new(
            codes::DRIFT_EXCEEDED,
            format!(
                "the picture drifted {} frames from the audio clock, {} s into the run",
                report::format_milli_frames(section.max_drift_milli_frames),
                section.worst_at_seconds
            ),
        )
        .with_detail("max_drift_milli_frames", section.max_drift_milli_frames)
        .with_detail("worst_at_seconds", section.worst_at_seconds));
    }
    Ok(())
}

/// Writes the report as pretty JSON, creating the parent directory.
fn write_report(out: &std::path::Path, report: &report::Report) -> SubResult<()> {
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).sub_context_with(codes::REPORT_UNWRITABLE, || {
            format!("cannot create {}", parent.display())
        })?;
    }
    let json = serde_json::to_string_pretty(report)
        .sub_context(codes::REPORT_UNWRITABLE, "the report would not serialise")?;
    std::fs::write(out, json + "\n").sub_context_with(codes::REPORT_UNWRITABLE, || {
        format!("cannot write {}", out.display())
    })
}

fn main() -> ExitCode {
    let _ = sub_core::logging::init(
        "subordinate_bench=info,sub_media=warn,sub_render=warn,sub_audio=warn,sub_edit=warn",
    );
    let args: Vec<String> = std::env::args().skip(1).collect();
    let outcome = parse(&args).and_then(|command| match command {
        Command::Help => {
            print!("{USAGE}");
            Ok(())
        }
        Command::Measure { out, options } => measure(&out, &options),
        Command::Sync { out, options } => measure_sync(&out, &options),
    });
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[{}] {}", error.code, error.message);
            if error.code == codes::BAD_ARGUMENT {
                eprint!("{USAGE}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, DEFAULT_OUT, DEFAULT_SYNC_OUT, codes, parse};
    use std::path::PathBuf;
    use sub_media::HardwarePreference;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn measure(values: &[&str]) -> (PathBuf, crate::run::Options) {
        match parse(&args(values)).expect("the arguments parse") {
            Command::Measure { out, options } => (out, options),
            other => panic!("expected a measurement, got {other:?}"),
        }
    }

    fn sync(values: &[&str]) -> (PathBuf, crate::sync::Options) {
        match parse(&args(values)).expect("the arguments parse") {
            Command::Sync { out, options } => (out, options),
            other => panic!("expected a sync run, got {other:?}"),
        }
    }

    #[test]
    fn no_arguments_measures_with_the_defaults() {
        let (out, options) = measure(&[]);
        assert_eq!(out, PathBuf::from(DEFAULT_OUT));
        assert_eq!(options, crate::run::Options::default());
    }

    #[test]
    fn counts_and_paths_override_the_defaults() {
        let (out, options) = measure(&[
            "--out",
            "/tmp/perf.json",
            "--frames",
            "10",
            "--seeks",
            "5",
            "--warmup",
            "0",
            "--fixtures",
            "/media",
        ]);
        assert_eq!(out, PathBuf::from("/tmp/perf.json"));
        assert_eq!(options.frames, 10);
        assert_eq!(options.seeks, 5);
        assert_eq!(options.warmup, 0);
        assert_eq!(options.fixtures_dir, Some(PathBuf::from("/media")));
    }

    #[test]
    fn the_switches_turn_off_hardware_decode_and_the_gpu() {
        let (_, options) = measure(&["--software", "--no-gpu"]);
        assert_eq!(options.hardware, HardwarePreference::Software);
        assert!(!options.use_gpu);
    }

    #[test]
    fn sync_measures_drift_into_its_own_report() {
        let (out, options) = sync(&["--sync"]);
        assert_eq!(out, PathBuf::from(DEFAULT_SYNC_OUT));
        assert_eq!(options, crate::sync::Options::default());
    }

    #[test]
    fn sync_takes_a_length_a_fixture_directory_and_software_decode() {
        let (out, options) = sync(&[
            "--sync",
            "--seconds",
            "30",
            "--fixtures",
            "/media",
            "--software",
            "--out",
            "/tmp/av-sync.json",
        ]);
        assert_eq!(out, PathBuf::from("/tmp/av-sync.json"));
        assert_eq!(options.seconds, Some(30));
        assert_eq!(options.fixtures_dir, Some(PathBuf::from("/media")));
        assert_eq!(options.hardware, HardwarePreference::Software);
    }

    #[test]
    fn a_run_length_without_the_sync_mode_is_rejected() {
        let error = parse(&args(&["--seconds", "30"])).expect_err("--seconds needs --sync");
        assert_eq!(error.code, codes::BAD_ARGUMENT);
    }

    #[test]
    fn help_is_asked_for_by_either_spelling() {
        assert_eq!(
            parse(&args(&["--help"])).expect("help parses"),
            Command::Help
        );
        assert_eq!(parse(&args(&["-h"])).expect("help parses"), Command::Help);
    }

    #[test]
    fn a_bad_argument_is_rejected_with_a_stable_code() {
        for bad in [
            vec!["--nope"],
            vec!["--frames"],
            vec!["--frames", "many"],
            vec!["--frames", "0"],
            vec!["--warmup", "-1"],
        ] {
            let error = parse(&args(&bad)).expect_err("the arguments are rejected");
            assert_eq!(error.code, codes::BAD_ARGUMENT, "for {bad:?}");
        }
    }
}
