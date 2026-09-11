//! Phase 5's exit criterion: can an hour of long-GOP camera footage be edited?
//!
//! The question the criterion asks is not how fast a decoder is, it is whether
//! the editor stays usable on material an hour long: the scrub bar has to keep
//! up with the hand dragging it, playback must not stop to think, and none of
//! it may eat the machine (docs/PLAN.md §5.2, §8). That is what proxies are
//! for, so this mode measures the proxy path end to end:
//!
//! * the one-hour long-GOP fixture is located, and an intra-only proxy is made
//!   from it through the production [`Proxy`] path — or reused, when one is
//!   already in the cache, exactly as the editor would;
//! * both files are then scrubbed and played through the same production
//!   decode and NV12 upload path the other scenarios use, so the numbers sit
//!   beside them in one report and the proxy's advantage is visible rather
//!   than asserted;
//! * the process's own peak resident size is read at the end and compared with
//!   a documented budget.
//!
//! Three integer comparisons come out of it — scrub rate against 30 fps,
//! playback stalls against zero, peak memory against the budget — and a run
//! whose numbers fail exits non-zero, like the A/V sync mode. A machine
//! without the fixture is a skipped section, not a failure.

use std::path::{Path, PathBuf};
use std::time::Instant;

use sub_core::{ResultExt as _, SubResult};
use sub_media::probe::NANOSECONDS;
use sub_media::{HardwarePreference, Proxy, ProxyCodec, ProxyOptions, ProxyScale, proxy_size};
use sub_model::ContentHash;
use sub_time::{Rational, RationalTime};

use crate::codes;
use crate::memory;
use crate::report::{Gpu, ProxySection, Scenario, ScenarioKind, ScenarioStatus};
use crate::run;

/// The source this mode is stated against: one hour of long-GOP 1080p.
pub const FIXTURE: &str = "longgop_1080p_1h.mp4";

/// The scrub rate the criterion asks for, in milli-fps.
pub const SCRUB_TARGET_MILLI_FPS: u64 = 30_000;

/// Seconds of timeline the playback scenario covers. The criterion is stated
/// over five seconds of playback.
pub const DEFAULT_PLAYBACK_SECONDS: u64 = 5;

/// Seeks the scrub scenario makes across the hour.
pub const DEFAULT_SEEKS: u64 = 60;

/// The documented memory budget, in mebibytes.
///
/// It is the frame cache's own default budget ([`sub_media::DEFAULT_BUDGET_BYTES`],
/// 512 MiB) with room around it for the decoder's buffers, the pictures in
/// flight, the upload staging and the GPU driver's own allocations: an editor
/// holding an hour-long source open must not grow with the length of that
/// source, so the budget is a constant, not a fraction of the media.
pub const DEFAULT_MEMORY_BUDGET_MIB: u64 = 2_048;

/// Where proxies are written when `--proxy-cache` is not given.
pub const DEFAULT_CACHE_DIR: &str = "target/bench/proxy-cache";

/// What one proxy validation run should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Where the fixtures live; `None` means the usual lookup, which honours
    /// `SUB_FIXTURES_DIR`.
    pub fixtures_dir: Option<PathBuf>,
    /// Where proxies are kept between runs; `None` means
    /// [`DEFAULT_CACHE_DIR`].
    pub cache_dir: Option<PathBuf>,
    /// Whether a hardware decoder may be preferred.
    pub hardware: HardwarePreference,
    /// Whether the texture stage runs at all.
    pub use_gpu: bool,
    /// Seeks timed per scrub scenario.
    pub seeks: u64,
    /// Seconds of timeline the playback scenario covers.
    pub seconds: u64,
    /// The memory budget the peak is judged against, in mebibytes.
    pub memory_budget_mib: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            fixtures_dir: None,
            cache_dir: None,
            hardware: HardwarePreference::Prefer,
            use_gpu: true,
            seeks: DEFAULT_SEEKS,
            seconds: DEFAULT_PLAYBACK_SECONDS,
            memory_budget_mib: DEFAULT_MEMORY_BUDGET_MIB,
        }
    }
}

/// Everything one proxy validation run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The criteria and what the proxy is.
    pub section: ProxySection,
    /// The four measured scenarios: source and proxy, playback and scrub.
    pub scenarios: Vec<Scenario>,
    /// The adapter the texture stage ran on, absent when there was none.
    pub gpu: Option<Gpu>,
}

/// The source fixture as the manifest describes it.
struct Source {
    /// Where the file is.
    path: PathBuf,
    /// Its exact length in nanoseconds.
    duration_nanos: u64,
    /// Its nominal frame rate.
    rate: Rational,
    /// Its coded picture size.
    width: u32,
    /// Its coded picture height.
    height: u32,
}

/// Validates editing the hour-long source with a proxy.
///
/// # Errors
///
/// Only genuine failures of the paths under test: a proxy that cannot be
/// made, a decoder that errors, a device that stops responding. A machine
/// without the fixture gets a skipped section instead, and a failing
/// criterion is reported in the section rather than raised here.
pub fn run(options: &Options) -> SubResult<Outcome> {
    let dir = options
        .fixtures_dir
        .clone()
        .unwrap_or_else(sub_test_support::fixtures_dir);
    let source = match locate(&dir) {
        Ok(found) => found,
        Err(reason) => {
            tracing::warn!(fixture = FIXTURE, %reason, "skipping the proxy validation");
            return Ok(Outcome {
                section: ProxySection::skipped(FIXTURE, &reason),
                scenarios: Vec::new(),
                gpu: None,
            });
        }
    };
    measure(&source, options)
}

/// The fixture and the facts about it the run needs, or why it cannot run.
fn locate(dir: &Path) -> Result<Source, String> {
    let path = sub_test_support::fixture_from(dir, FIXTURE).map_err(|error| error.to_string())?;
    let manifest = sub_test_support::load_manifest_from(dir).map_err(|error| error.to_string())?;
    let fixture = manifest
        .get(FIXTURE)
        .ok_or_else(|| format!("no fixture named '{FIXTURE}' in the manifest"))?;
    let (num, den) = fixture.fps();
    let rate = Rational::new(num, den)
        .ok_or_else(|| format!("fixture '{FIXTURE}' has no usable frame rate"))?;
    Ok(Source {
        path,
        duration_nanos: fixture.duration_ns,
        rate,
        width: fixture.width,
        height: fixture.height,
    })
}

/// One frame of `rate`, in nanoseconds: the budget a playback step has.
fn frame_interval_nanos(rate: Rational) -> u64 {
    let interval = RationalTime::from_frames(1, rate).rescaled_to(NANOSECONDS);
    u64::try_from(interval.value()).unwrap_or(0)
}

/// Makes or reuses the proxy, measures both files and judges the criteria.
fn measure(source: &Source, options: &Options) -> SubResult<Outcome> {
    let cache_dir = options
        .cache_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CACHE_DIR));
    std::fs::create_dir_all(&cache_dir).sub_context_with(codes::REPORT_UNWRITABLE, || {
        format!("cannot create the proxy cache {}", cache_dir.display())
    })?;
    let proxy_options = options_for(source);

    let hash = ContentHash::of_file(&source.path)?;
    let reused = Proxy::load(&cache_dir, hash, proxy_options).is_some();
    let started = Instant::now();
    let proxy = Proxy::generate(&source.path, &cache_dir, proxy_options)?;
    let generation_nanos = if reused {
        0
    } else {
        u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
    };

    let interval = frame_interval_nanos(source.rate);
    let frames = frames_in(options.seconds, source.rate);
    let run_options = run::Options {
        fixtures_dir: options.fixtures_dir.clone(),
        frames,
        seeks: options.seeks,
        warmup: run::Options::default().warmup,
        hardware: options.hardware,
        use_gpu: options.use_gpu,
        stall_threshold_nanos: Some(interval),
    };
    let context = if options.use_gpu {
        run::open_adapter()?
    } else {
        None
    };
    let gpu = context.as_ref().map(|context| Gpu {
        adapter: context.describe(),
        software: context.is_software(),
    });
    let context = context.as_ref();

    let proxy_path = proxy.path();
    let proxy_name = file_name(&proxy_path);
    let scenarios = vec![
        run::measure_playback(&source.path, FIXTURE, &run_options, context)?,
        run::measure_scrub(
            &source.path,
            FIXTURE,
            source.duration_nanos,
            &run_options,
            context,
        )?,
        run::measure_playback(&proxy_path, &proxy_name, &run_options, context)?,
        run::measure_scrub(
            &proxy_path,
            &proxy_name,
            source.duration_nanos,
            &run_options,
            context,
        )?,
    ];

    let section = ProxySection {
        source: FIXTURE.to_owned(),
        status: ScenarioStatus::Measured,
        skipped_reason: None,
        proxy: Some(proxy_name.clone()),
        proxy_codec: Some(proxy_options.codec.as_str().to_owned()),
        proxy_scale: Some(proxy_options.scale.as_str().to_owned()),
        proxy_width: proxy.width(),
        proxy_height: proxy.height(),
        proxy_frames: u64::try_from(proxy.frame_count()).unwrap_or(u64::MAX),
        source_bytes: file_size(&source.path),
        proxy_bytes: file_size(&proxy_path),
        generation_nanos,
        generated: !reused,
        playback_seconds: options.seconds,
        frame_interval_nanos: interval,
        scrub_target_milli_fps: SCRUB_TARGET_MILLI_FPS,
        proxy_scrub_milli_fps: rate_of(&scenarios, &proxy_name, ScenarioKind::Scrub),
        source_scrub_milli_fps: rate_of(&scenarios, FIXTURE, ScenarioKind::Scrub),
        proxy_playback_stalls: find(&scenarios, &proxy_name, ScenarioKind::Playback)
            .and_then(|scenario| scenario.stalls),
        proxy_playback_max_nanos: find(&scenarios, &proxy_name, ScenarioKind::Playback)
            .and_then(|scenario| scenario.decode_to_texture.or(scenario.decode))
            .map(|stats| stats.max_nanos),
        memory_budget_bytes: options.memory_budget_mib * 1024 * 1024,
        peak_rss_bytes: memory::peak_rss_bytes(),
    };
    Ok(Outcome {
        section,
        scenarios,
        gpu,
    })
}

/// How the proxy is made: half resolution, in whichever intra-only codec this
/// installation can actually write at that size.
fn options_for(source: &Source) -> ProxyOptions {
    let scale = ProxyScale::Half;
    let (width, height) = proxy_size(source.width, source.height, scale);
    ProxyOptions {
        codec: ProxyCodec::preferred_at(width, height),
        scale,
        ..ProxyOptions::default()
    }
}

/// How many frames `seconds` of timeline holds at `rate`, at least one.
fn frames_in(seconds: u64, rate: Rational) -> u64 {
    let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
    let frames = RationalTime::from_seconds(seconds)
        .rescaled_to(rate)
        .value();
    u64::try_from(frames).unwrap_or(0).max(1)
}

/// The scenario for a file and kind, if the run produced one.
fn find<'a>(scenarios: &'a [Scenario], fixture: &str, kind: ScenarioKind) -> Option<&'a Scenario> {
    scenarios
        .iter()
        .find(|scenario| scenario.fixture == fixture && scenario.kind == kind)
}

/// The sustained rate of one scenario, in milli-fps.
fn rate_of(scenarios: &[Scenario], fixture: &str, kind: ScenarioKind) -> Option<u64> {
    find(scenarios, fixture, kind).and_then(|scenario| scenario.sustained_milli_fps)
}

/// A path's file name, or its whole display form when it has none.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// How many bytes a file holds, zero when it cannot be read.
fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_MEMORY_BUDGET_MIB, FIXTURE, Options, SCRUB_TARGET_MILLI_FPS, Source, file_name,
        frame_interval_nanos, frames_in, options_for,
    };
    use std::path::{Path, PathBuf};
    use sub_media::{ProxyScale, proxy_size};
    use sub_time::Rational;

    fn source(width: u32, height: u32) -> Source {
        Source {
            path: PathBuf::from("/media/take-1.mp4"),
            duration_nanos: 3_600_000_000_000,
            rate: Rational::FPS_25,
            width,
            height,
        }
    }

    #[test]
    fn the_measured_source_is_the_one_hour_long_gop_fixture() {
        assert_eq!(FIXTURE, "longgop_1080p_1h.mp4");
    }

    #[test]
    fn the_defaults_cover_five_seconds_of_playback_and_a_documented_budget() {
        let options = Options::default();
        assert_eq!(options.seconds, 5);
        assert!(options.seeks > 1);
        assert_eq!(options.memory_budget_mib, DEFAULT_MEMORY_BUDGET_MIB);
        assert_eq!(SCRUB_TARGET_MILLI_FPS, 30_000);
    }

    #[test]
    fn a_frame_interval_is_exact_at_both_integer_and_fractional_rates() {
        assert_eq!(frame_interval_nanos(Rational::FPS_25), 40_000_000);
        let ntsc = Rational::new(30_000, 1_001).expect("29.97 is a rate");
        // 1001 / 30000 s, to the nearest nanosecond.
        assert_eq!(frame_interval_nanos(ntsc), 33_366_667);
    }

    #[test]
    fn five_seconds_at_twenty_five_frames_is_one_hundred_and_twenty_five_frames() {
        assert_eq!(frames_in(5, Rational::FPS_25), 125);
        let ntsc = Rational::new(30_000, 1_001).expect("29.97 is a rate");
        assert_eq!(frames_in(5, ntsc), 150);
        // Never zero: a run always times at least one frame.
        assert_eq!(frames_in(0, Rational::FPS_25), 1);
    }

    #[test]
    fn a_proxy_is_half_resolution_in_a_codec_this_installation_can_write() {
        let options = options_for(&source(1920, 1080));
        assert_eq!(options.scale, ProxyScale::Half);
        assert_eq!(proxy_size(1920, 1080, options.scale), (960, 540));
        assert!(options.validate().is_ok());
    }

    #[test]
    fn a_scenario_is_named_by_the_file_it_measured() {
        assert_eq!(
            file_name(Path::new("/cache/proxy-abc.mov")),
            "proxy-abc.mov"
        );
        assert_eq!(file_name(Path::new("/")), "/");
    }
}
