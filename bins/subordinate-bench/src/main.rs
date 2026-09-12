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
//! `--proxy` runs the third measurement: the proxy editing validation of
//! [`proxy`], which makes an intra-only proxy of the one-hour long-GOP fixture
//! and measures scrubbing, playback and peak memory on both files. Like
//! `--sync` it can fail on a number rather than on the machine.
//!
//! ```text
//! subordinate-bench [--out PATH] [--frames N] [--seeks N] [--warmup N]
//!                   [--fixtures DIR] [--software] [--no-gpu]
//! subordinate-bench --sync [--out PATH] [--seconds N] [--fixtures DIR]
//!                   [--software]
//! subordinate-bench --proxy [--out PATH] [--seconds N] [--seeks N]
//!                   [--fixtures DIR] [--proxy-cache DIR]
//!                   [--memory-budget MIB] [--software] [--no-gpu]
//! ```
//!
//! The harness never fails a build because of the machine it runs on: a
//! missing fixture is a skipped scenario and a machine with no wgpu adapter
//! measures decode alone, both recorded in the report.

use std::path::PathBuf;
use std::process::ExitCode;

use sub_core::{ResultExt as _, SubError, SubResult};
use sub_media::HardwarePreference;

mod memory;
mod proxy;
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
    /// Editing the hour-long source with a proxy missed one of phase 5's
    /// numbers: the scrub rate, a stall-free playback, or the memory budget.
    pub const PROXY_BELOW_TARGET: ErrorCode = ErrorCode::from_static("bench.proxy_below_target");
}

/// Where the report goes when `--out` is not given.
const DEFAULT_OUT: &str = "target/bench/perf.json";

/// Where the sync report goes when `--out` is not given.
const DEFAULT_SYNC_OUT: &str = "target/bench/av-sync.json";

/// Where the proxy report goes when `--out` is not given.
const DEFAULT_PROXY_OUT: &str = "target/bench/proxy.json";

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
  --legacy-scrub    measure the seek path as it was before TASK-133: no cache
                    of the pictures a step decoded, and no allowance for what
                    a flush costs when a step chooses between decoding on and
                    seeking. This is how a before-and-after is taken on a
                    machine without rebuilding anything.
  --sync            measure A/V sync and drift over the long fixture instead
                    (default report: target/bench/av-sync.json); exits
                    non-zero when the picture drifts a whole frame or more
  --proxy           validate editing the one-hour long-GOP fixture with a
                    proxy instead (default report: target/bench/proxy.json);
                    exits non-zero when the scrub rate, the stall count or
                    the peak memory misses phase 5's numbers
  --seconds N       with --sync, stop after N seconds of timeline; with
                    --proxy, seconds of timeline played (default: 5)
  --proxy-cache DIR with --proxy, keep the generated proxy here
  --memory-budget N with --proxy, the memory budget in MiB (default: 2048)
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
    /// Validate editing the hour-long source with a proxy instead.
    Proxy {
        /// Where the JSON report goes.
        out: PathBuf,
        /// How the run is configured.
        options: proxy::Options,
    },
}

/// Parses the command line.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENT`] for an unknown option, a missing value, or a count
/// that is not a positive integer.
fn parse(args: &[String]) -> SubResult<Command> {
    choose(scan(args)?)
}

/// Which measurement the command line asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The scrub and playback scenarios, which is what no mode switch means.
    Measure,
    /// `--sync`: A/V sync and drift.
    Sync,
    /// `--proxy`: editing the hour-long source with a proxy.
    Proxy,
    /// `-h` or `--help`: print the usage text and measure nothing.
    Help,
}

impl Mode {
    /// The switch that asks for this mode, for an error message.
    fn as_switch(self) -> &'static str {
        match self {
            Self::Measure => "the default run",
            Self::Sync => "--sync",
            Self::Proxy => "--proxy",
            Self::Help => "--help",
        }
    }
}

/// The switches one command line carried, before a mode is chosen from them.
struct Parsed {
    /// Where the report should go, when `--out` was given.
    out: Option<PathBuf>,
    /// The scrub and playback options, which also carry the settings every
    /// mode shares.
    options: run::Options,
    /// The measurement asked for.
    mode: Mode,
    /// The value of `--seconds`.
    seconds: Option<u64>,
    /// Whether `--seeks` was given, which decides whether a mode's own
    /// default applies instead.
    seeks_given: bool,
    /// The value of `--proxy-cache`.
    proxy_cache: Option<PathBuf>,
    /// The value of `--memory-budget`, in mebibytes.
    memory_budget: Option<u64>,
}

/// Reads the command line into [`Parsed`], rejecting what it cannot read.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENT`] for an unknown option, a missing value, or a count
/// that is not a positive integer.
fn scan(args: &[String]) -> SubResult<Parsed> {
    let mut parsed = Parsed {
        out: None,
        options: run::Options::default(),
        mode: Mode::Measure,
        seconds: None,
        seeks_given: false,
        proxy_cache: None,
        memory_budget: None,
    };
    let mut index = 0;

    while index < args.len() {
        let arg = args[index].as_str();
        index += 1;
        match arg {
            "-h" | "--help" => {
                parsed.mode = Mode::Help;
                return Ok(parsed);
            }
            "--software" => parsed.options.hardware = HardwarePreference::Software,
            "--no-gpu" => parsed.options.use_gpu = false,
            "--legacy-scrub" => parsed.options.legacy_scrub = true,
            "--sync" => parsed.mode = one_mode(parsed.mode, Mode::Sync)?,
            "--proxy" => parsed.mode = one_mode(parsed.mode, Mode::Proxy)?,
            "--seconds" => parsed.seconds = Some(count(args, &mut index, arg)?),
            "--memory-budget" => parsed.memory_budget = Some(count(args, &mut index, arg)?),
            "--out" => parsed.out = Some(PathBuf::from(value(args, &mut index, arg)?)),
            "--proxy-cache" => {
                parsed.proxy_cache = Some(PathBuf::from(value(args, &mut index, arg)?));
            }
            "--fixtures" => {
                parsed.options.fixtures_dir = Some(PathBuf::from(value(args, &mut index, arg)?));
            }
            "--frames" => parsed.options.frames = count(args, &mut index, arg)?,
            "--seeks" => {
                parsed.options.seeks = count(args, &mut index, arg)?;
                parsed.seeks_given = true;
            }
            "--warmup" => {
                let raw = value(args, &mut index, arg)?;
                parsed.options.warmup = raw
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
    Ok(parsed)
}

/// `wanted` when no other measurement has been asked for already.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENT`] when the command line asks for two measurements at
/// once, which would leave it ambiguous which numbers were wanted.
fn one_mode(current: Mode, wanted: Mode) -> SubResult<Mode> {
    if current == Mode::Measure || current == wanted {
        return Ok(wanted);
    }
    Err(SubError::new(
        codes::BAD_ARGUMENT,
        format!(
            "{} and {} are separate runs; ask for one of them",
            current.as_switch(),
            wanted.as_switch()
        ),
    ))
}

/// The run those switches ask for.
///
/// # Errors
///
/// [`codes::BAD_ARGUMENT`] when two modes were asked for at once, or when an
/// option belongs to a mode that was not asked for.
fn choose(parsed: Parsed) -> SubResult<Command> {
    let Parsed {
        out,
        options,
        mode,
        seconds,
        seeks_given,
        proxy_cache,
        memory_budget,
    } = parsed;
    if mode == Mode::Help {
        return Ok(Command::Help);
    }
    if mode == Mode::Proxy {
        let defaults = proxy::Options::default();
        return Ok(Command::Proxy {
            out: out.unwrap_or_else(|| PathBuf::from(DEFAULT_PROXY_OUT)),
            options: proxy::Options {
                fixtures_dir: options.fixtures_dir,
                cache_dir: proxy_cache,
                hardware: options.hardware,
                use_gpu: options.use_gpu,
                seeks: if seeks_given {
                    options.seeks
                } else {
                    defaults.seeks
                },
                seconds: seconds.unwrap_or(defaults.seconds),
                memory_budget_mib: memory_budget.unwrap_or(defaults.memory_budget_mib),
            },
        });
    }
    if let Some(dir) = proxy_cache {
        return Err(SubError::new(
            codes::BAD_ARGUMENT,
            format!(
                "--proxy-cache only applies to --proxy, not to '{}'",
                dir.display()
            ),
        ));
    }
    if memory_budget.is_some() {
        return Err(SubError::new(
            codes::BAD_ARGUMENT,
            "--memory-budget only applies to --proxy",
        ));
    }
    if mode == Mode::Sync {
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
            "--seconds only applies to --sync or --proxy",
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

/// Runs the proxy editing validation, writes the report and prints the
/// summary.
///
/// # Errors
///
/// [`codes::PROXY_BELOW_TARGET`] when a measured criterion failed — the scrub
/// rate, a stall-free playback or the memory budget — plus whatever the
/// measurement or the report writer reports. A machine without the one-hour
/// fixture is a skipped section, not an error.
fn measure_proxy(out: &std::path::Path, options: &proxy::Options) -> SubResult<()> {
    let outcome = proxy::run(options)?;
    let mut report = report::Report::new(outcome.gpu);
    report.scenarios = outcome.scenarios;
    report.proxy = Some(outcome.section.clone());
    write_report(out, &report)?;

    for line in report.summary_lines() {
        println!("{line}");
    }
    println!("report       {}", out.display());
    if outcome.section.status == report::ScenarioStatus::Skipped {
        println!(
            "the proxy validation was not measured: generate the one-hour fixture with \
             scripts/gen-fixtures.sh --hour first"
        );
        return Ok(());
    }
    let failures = outcome.section.failures();
    if failures.is_empty() {
        return Ok(());
    }
    Err(
        SubError::new(codes::PROXY_BELOW_TARGET, failures.join("; "))
            .with_detail("failed_criteria", failures.len()),
    )
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
        Command::Proxy { out, options } => measure_proxy(&out, &options),
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
    use super::{Command, DEFAULT_OUT, DEFAULT_PROXY_OUT, DEFAULT_SYNC_OUT, codes, parse};
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

    fn proxy(values: &[&str]) -> (PathBuf, crate::proxy::Options) {
        match parse(&args(values)).expect("the arguments parse") {
            Command::Proxy { out, options } => (out, options),
            other => panic!("expected a proxy run, got {other:?}"),
        }
    }

    #[test]
    fn proxy_validates_editing_into_its_own_report() {
        let (out, options) = proxy(&["--proxy"]);
        assert_eq!(out, PathBuf::from(DEFAULT_PROXY_OUT));
        assert_eq!(options, crate::proxy::Options::default());
    }

    #[test]
    fn proxy_takes_a_length_a_seek_count_a_cache_and_a_budget() {
        let (out, options) = proxy(&[
            "--proxy",
            "--seconds",
            "10",
            "--seeks",
            "12",
            "--proxy-cache",
            "/cache",
            "--memory-budget",
            "512",
            "--software",
            "--no-gpu",
            "--out",
            "/tmp/proxy.json",
        ]);
        assert_eq!(out, PathBuf::from("/tmp/proxy.json"));
        assert_eq!(options.seconds, 10);
        assert_eq!(options.seeks, 12);
        assert_eq!(options.cache_dir, Some(PathBuf::from("/cache")));
        assert_eq!(options.memory_budget_mib, 512);
        assert_eq!(options.hardware, HardwarePreference::Software);
        assert!(!options.use_gpu);
    }

    #[test]
    fn the_two_measuring_modes_are_not_asked_for_together() {
        let error = parse(&args(&["--sync", "--proxy"])).expect_err("one mode at a time");
        assert_eq!(error.code, codes::BAD_ARGUMENT);
    }

    #[test]
    fn the_proxy_only_options_are_rejected_elsewhere() {
        for bad in [
            vec!["--proxy-cache", "/cache"],
            vec!["--memory-budget", "512"],
            vec!["--sync", "--memory-budget", "512"],
        ] {
            let error = parse(&args(&bad)).expect_err("the arguments are rejected");
            assert_eq!(error.code, codes::BAD_ARGUMENT, "for {bad:?}");
        }
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
