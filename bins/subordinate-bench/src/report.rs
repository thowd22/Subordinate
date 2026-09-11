//! The JSON shape the harness writes and the summary it prints.
//!
//! The document is stable: fields are only added, never renamed or given a new
//! meaning, so a later run can be compared against a recorded baseline in
//! `docs/PERFORMANCE.md`. Every duration is an exact nanosecond count and
//! every rate is in milli-frames per second (see [`crate::stats`]).

use serde::{Deserialize, Serialize};
use sub_time::Rational;

use crate::stats::{Stats, format_milli_fps, format_millis, rate_milli_fps};

/// Schema version of the JSON report.
pub const REPORT_VERSION: u32 = 1;

/// What a scenario measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioKind {
    /// Sequential decode of consecutive frames, as playback does.
    Playback,
    /// Repeated seeks across the whole file, as dragging the scrub bar does.
    Scrub,
}

impl ScenarioKind {
    /// The word used in the report and the summary.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Playback => "playback",
            Self::Scrub => "scrub",
        }
    }
}

/// Whether a scenario produced numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioStatus {
    /// The scenario ran and its statistics are populated.
    Measured,
    /// The scenario could not run here; `skipped_reason` says why.
    Skipped,
}

/// The machine the numbers were taken on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// Target operating system, for example `linux`.
    pub os: String,
    /// Target architecture, for example `x86_64`.
    pub arch: String,
    /// Cargo profile the harness itself was built with. Debug numbers are
    /// only ever compared against other debug numbers.
    pub profile: String,
}

/// The wgpu adapter the texture stage ran on, when there was one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpu {
    /// The adapter as `sub_render::RenderContext::describe` reports it.
    pub adapter: String,
    /// True for a CPU renderer, which caps what the numbers say about real
    /// hardware.
    pub software: bool,
}

/// One measured (or skipped) scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    /// Fixture file name, for example `bars_2160p_h264.mp4`.
    pub fixture: String,
    /// What the scenario measures.
    pub kind: ScenarioKind,
    /// Whether numbers were produced.
    pub status: ScenarioStatus,
    /// Why a skipped scenario was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
    /// Picture width in pixels, zero when the scenario was skipped.
    pub width: u32,
    /// Picture height in pixels, zero when the scenario was skipped.
    pub height: u32,
    /// The GStreamer decoder element the pipeline chose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decoder: Option<String>,
    /// How many frames were timed, warm-up excluded.
    pub frames: u64,
    /// Wall time of the timed frames together.
    pub wall_nanos: u64,
    /// Decode alone: `next_frame` for playback, `seek_to` for scrub.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decode: Option<Stats>,
    /// Both plane uploads plus the YUV-to-RGB pass, waited to GPU completion.
    /// Absent when the machine had no usable adapter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload: Option<Stats>,
    /// Decode-to-texture: the two stages above per frame. This is the latency
    /// the viewer pays before a scrubbed frame can be shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decode_to_texture: Option<Stats>,
    /// Frames per second sustained across the whole run, in milli-fps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sustained_milli_fps: Option<u64>,
    /// How long a step could take before it counted as a stall, in
    /// nanoseconds. Present only for a scenario that counted stalls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stall_threshold_nanos: Option<u64>,
    /// Timed steps that took longer than that threshold: frames that would
    /// not have been on screen in time. Present only when stalls were
    /// counted; zero is the passing value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stalls: Option<u64>,
}

impl Scenario {
    /// A scenario that could not run on this machine.
    pub fn skipped(fixture: &str, kind: ScenarioKind, reason: impl Into<String>) -> Self {
        Self {
            fixture: fixture.to_owned(),
            kind,
            status: ScenarioStatus::Skipped,
            skipped_reason: Some(reason.into()),
            width: 0,
            height: 0,
            decoder: None,
            frames: 0,
            wall_nanos: 0,
            decode: None,
            upload: None,
            decode_to_texture: None,
            sustained_milli_fps: None,
            stall_threshold_nanos: None,
            stalls: None,
        }
    }

    /// The one line the summary prints for this scenario.
    pub fn summary_line(&self) -> String {
        if self.status == ScenarioStatus::Skipped {
            return format!(
                "{:<10} {:<24} skipped: {}",
                self.kind.as_str(),
                self.fixture,
                self.skipped_reason.as_deref().unwrap_or("no reason given")
            );
        }
        let latency = self.decode_to_texture.or(self.decode);
        let (p50, p95) = latency.map_or_else(
            || ("n/a".to_owned(), "n/a".to_owned()),
            |stats| {
                (
                    format_millis(stats.p50_nanos),
                    format_millis(stats.p95_nanos),
                )
            },
        );
        format!(
            "{:<10} {:<24} {}x{:<5} {:>4} frames  {:>8} fps  p50 {:>10}  p95 {:>10}{}{}",
            self.kind.as_str(),
            self.fixture,
            self.width,
            self.height,
            self.frames,
            format_milli_fps(self.sustained_milli_fps),
            p50,
            p95,
            match self.stalls {
                Some(stalls) => format!("  {stalls} stalls"),
                None => String::new(),
            },
            if self.upload.is_none() {
                "  [decode only: no GPU]"
            } else {
                ""
            }
        )
    }
}

/// What the A/V sync and drift harness measured (or why it could not run).
///
/// Drift is counted in milli-frames -- `1_000` is one whole frame -- so the
/// phase 3 exit criterion, "under one frame", is an exact integer comparison
/// and no number here is a float.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSection {
    /// Fixture file name, the ten-minute long-GOP clip.
    pub fixture: String,
    /// Whether numbers were produced.
    pub status: ScenarioStatus,
    /// Why a skipped run was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
    /// The GStreamer decoder element the pipeline chose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decoder: Option<String>,
    /// Numerator of the video timebase the drift is counted in frames of.
    pub video_rate_num: u32,
    /// Denominator of that timebase.
    pub video_rate_den: u32,
    /// Sequence sample rate the mixer rendered at.
    pub sequence_sample_rate: u32,
    /// Device sample rate the callback converted to.
    pub device_sample_rate: u32,
    /// Device frames per callback block.
    pub block_frames: u64,
    /// Seconds of timeline the run was asked to play.
    pub seconds_requested: u64,
    /// Seconds of timeline the audio clock actually reached.
    pub seconds_played: u64,
    /// How many one-second samples were taken.
    pub samples: u64,
    /// Frames put on screen over the run.
    pub frames_shown: u64,
    /// Frames the decoder delivered.
    pub frames_decoded: u64,
    /// Presentations the scheduler dropped because the master ran past them.
    pub dropped_frames: u64,
    /// The worst drift seen, in milli-frames: zero while the frame on screen
    /// is the one the audible samples belong to, otherwise the signed
    /// distance beyond that frame's own interval. Positive means the picture
    /// lagged the sound.
    pub max_drift_milli_frames: i64,
    /// The raw offset at that sample: audible position minus the PTS of the
    /// frame on screen, in milli-frames.
    pub max_offset_milli_frames: i64,
    /// Whole seconds into the run at which the worst sample was taken.
    pub worst_at_seconds: u64,
}

impl SyncSection {
    /// A run that could not happen on this machine.
    pub fn skipped(fixture: &str, reason: impl Into<String>) -> Self {
        Self {
            fixture: fixture.to_owned(),
            status: ScenarioStatus::Skipped,
            skipped_reason: Some(reason.into()),
            decoder: None,
            video_rate_num: 0,
            video_rate_den: 1,
            sequence_sample_rate: 0,
            device_sample_rate: 0,
            block_frames: 0,
            seconds_requested: 0,
            seconds_played: 0,
            samples: 0,
            frames_shown: 0,
            frames_decoded: 0,
            dropped_frames: 0,
            max_drift_milli_frames: 0,
            max_offset_milli_frames: 0,
            worst_at_seconds: 0,
        }
    }

    /// A run about to be measured, carrying the shape it will run at.
    pub fn measured(fixture: &str, video_rate: Rational, seconds: u64) -> Self {
        Self {
            status: ScenarioStatus::Measured,
            skipped_reason: None,
            video_rate_num: video_rate.numerator(),
            video_rate_den: video_rate.denominator(),
            sequence_sample_rate: crate::sync::SEQUENCE_RATE,
            device_sample_rate: crate::sync::DEVICE_RATE,
            block_frames: crate::sync::BLOCK_FRAMES as u64,
            seconds_requested: seconds,
            ..Self::skipped(fixture, "not measured")
        }
    }

    /// True when the picture never got a whole frame away from the sound,
    /// which is phase 3's exit criterion.
    pub fn within_one_frame(&self) -> bool {
        self.max_drift_milli_frames.abs() < crate::sync::FRAME_MILLI
    }

    /// The lines the summary prints for the sync run.
    pub fn summary_lines(&self) -> Vec<String> {
        if self.status == ScenarioStatus::Skipped {
            return vec![format!(
                "av-sync    {:<24} skipped: {}",
                self.fixture,
                self.skipped_reason.as_deref().unwrap_or("no reason given")
            )];
        }
        vec![
            format!(
                "av-sync    {:<24} {} s played  {} samples  {} frames shown  {} dropped",
                self.fixture,
                self.seconds_played,
                self.samples,
                self.frames_shown,
                self.dropped_frames
            ),
            format!(
                "av-sync    max drift {} frames at {} s (offset {} frames){}",
                format_milli_frames(self.max_drift_milli_frames),
                self.worst_at_seconds,
                format_milli_frames(self.max_offset_milli_frames),
                if self.within_one_frame() {
                    ""
                } else {
                    "  [OVER ONE FRAME]"
                }
            ),
        ]
    }
}

/// What the proxy editing validation measured (or why it could not run).
///
/// Phase 5's exit criterion is stated about editing, not about decoding: a
/// one-hour long-GOP source, a proxy made from it, and an editor that scrubs
/// above 30 fps, never stalls while playing and stays inside a memory budget
/// (docs/PLAN.md §8). Each of those is an integer comparison here; the
/// per-scenario numbers behind them are ordinary [`Scenario`] entries in the
/// same report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxySection {
    /// Fixture file name of the long-GOP source.
    pub source: String,
    /// Whether numbers were produced.
    pub status: ScenarioStatus,
    /// Why a skipped run was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
    /// File name of the proxy that was measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// The intra-only codec the proxy was written in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_codec: Option<String>,
    /// How far the proxy was scaled down.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_scale: Option<String>,
    /// Proxy picture width in pixels.
    pub proxy_width: u32,
    /// Proxy picture height in pixels.
    pub proxy_height: u32,
    /// Frames the proxy holds, from its manifest.
    pub proxy_frames: u64,
    /// Bytes the source file occupies.
    pub source_bytes: u64,
    /// Bytes the proxy file occupies.
    pub proxy_bytes: u64,
    /// Wall time the proxy took to make, in nanoseconds. Zero when a proxy
    /// already in the cache was reused.
    pub generation_nanos: u64,
    /// True when this run transcoded the proxy rather than reusing one.
    pub generated: bool,
    /// Seconds of timeline the playback scenario covered.
    pub playback_seconds: u64,
    /// One frame interval of the source, in nanoseconds: the budget a
    /// playback step has to fit inside.
    pub frame_interval_nanos: u64,
    /// The scrub rate the criterion asks for, in milli-fps.
    pub scrub_target_milli_fps: u64,
    /// Scrub rate the proxy sustained, in milli-fps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_scrub_milli_fps: Option<u64>,
    /// Scrub rate the original sustained, for comparison.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_scrub_milli_fps: Option<u64>,
    /// Playback steps on the proxy that overran a frame interval.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_playback_stalls: Option<u64>,
    /// The slowest playback step on the proxy, in nanoseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_playback_max_nanos: Option<u64>,
    /// The documented memory budget for the run, in bytes.
    pub memory_budget_bytes: u64,
    /// Peak resident set size of the process, in bytes, or `None` where this
    /// platform does not report one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_rss_bytes: Option<u64>,
}

impl ProxySection {
    /// A run that could not happen on this machine.
    pub fn skipped(source: &str, reason: impl Into<String>) -> Self {
        Self {
            source: source.to_owned(),
            status: ScenarioStatus::Skipped,
            skipped_reason: Some(reason.into()),
            proxy: None,
            proxy_codec: None,
            proxy_scale: None,
            proxy_width: 0,
            proxy_height: 0,
            proxy_frames: 0,
            source_bytes: 0,
            proxy_bytes: 0,
            generation_nanos: 0,
            generated: false,
            playback_seconds: 0,
            frame_interval_nanos: 0,
            scrub_target_milli_fps: 0,
            proxy_scrub_milli_fps: None,
            source_scrub_milli_fps: None,
            proxy_playback_stalls: None,
            proxy_playback_max_nanos: None,
            memory_budget_bytes: 0,
            peak_rss_bytes: None,
        }
    }

    /// True when scrubbing the proxy reached the rate the criterion asks for.
    pub fn scrub_ok(&self) -> bool {
        self.proxy_scrub_milli_fps
            .is_some_and(|rate| rate >= self.scrub_target_milli_fps)
    }

    /// True when no playback step on the proxy overran its frame interval.
    pub fn playback_ok(&self) -> bool {
        self.proxy_playback_stalls == Some(0)
    }

    /// True when the process stayed inside the memory budget.
    ///
    /// A platform that reports no peak counts as passing: the criterion is
    /// then simply unmeasured, and the section says so by carrying no
    /// `peak_rss_bytes`.
    pub fn memory_ok(&self) -> bool {
        self.peak_rss_bytes
            .is_none_or(|peak| peak <= self.memory_budget_bytes)
    }

    /// The criteria this run failed, empty when it passed or was skipped.
    pub fn failures(&self) -> Vec<String> {
        if self.status == ScenarioStatus::Skipped {
            return Vec::new();
        }
        let mut failures = Vec::new();
        if !self.scrub_ok() {
            failures.push(format!(
                "proxy scrub reached {} fps, under the {} fps the criterion asks for",
                format_milli_fps(self.proxy_scrub_milli_fps),
                format_milli_fps(Some(self.scrub_target_milli_fps))
            ));
        }
        if !self.playback_ok() {
            failures.push(match self.proxy_playback_stalls {
                Some(stalls) => format!(
                    "{stalls} of the timed playback steps overran a {} frame interval",
                    format_millis(self.frame_interval_nanos)
                ),
                None => "playback was not measured".to_owned(),
            });
        }
        if !self.memory_ok() {
            failures.push(format!(
                "peak memory was {}, over the {} budget",
                format_mib(self.peak_rss_bytes.unwrap_or(0)),
                format_mib(self.memory_budget_bytes)
            ));
        }
        failures
    }

    /// The lines the summary prints for the proxy run.
    pub fn summary_lines(&self) -> Vec<String> {
        if self.status == ScenarioStatus::Skipped {
            return vec![format!(
                "proxy      {:<24} skipped: {}",
                self.source,
                self.skipped_reason.as_deref().unwrap_or("no reason given")
            )];
        }
        let mut lines = vec![
            format!(
                "proxy      {:<24} {}x{} {} ({}), {} frames, {} -> {}{}",
                self.source,
                self.proxy_width,
                self.proxy_height,
                self.proxy_codec.as_deref().unwrap_or("unknown codec"),
                self.proxy_scale.as_deref().unwrap_or("unknown scale"),
                self.proxy_frames,
                format_mib(self.source_bytes),
                format_mib(self.proxy_bytes),
                if self.generated {
                    format!(", made in {}", format_millis(self.generation_nanos))
                } else {
                    ", reused from the cache".to_owned()
                }
            ),
            format!(
                "proxy      scrub {} fps (target {}) [{}]  playback {} stalls over {} s [{}]",
                format_milli_fps(self.proxy_scrub_milli_fps),
                format_milli_fps(Some(self.scrub_target_milli_fps)),
                verdict(self.scrub_ok()),
                self.proxy_playback_stalls
                    .map_or_else(|| "n/a".to_owned(), |stalls| stalls.to_string()),
                self.playback_seconds,
                verdict(self.playback_ok()),
            ),
            format!(
                "proxy      peak memory {} of a {} budget [{}]",
                self.peak_rss_bytes
                    .map_or_else(|| "unmeasured".to_owned(), format_mib),
                format_mib(self.memory_budget_bytes),
                verdict(self.memory_ok()),
            ),
        ];
        for failure in self.failures() {
            lines.push(format!("proxy      FAILED: {failure}"));
        }
        lines
    }
}

/// `PASS` or `FAIL`, for a criterion line.
fn verdict(ok: bool) -> &'static str {
    if ok { "PASS" } else { "FAIL" }
}

/// A byte count as whole mebibytes, rounded down.
pub fn format_mib(bytes: u64) -> String {
    format!("{} MiB", bytes / (1024 * 1024))
}

/// A milli-frame count as frames with three decimal places, sign included.
pub fn format_milli_frames(milli: i64) -> String {
    let sign = if milli < 0 { "-" } else { "" };
    let magnitude = milli.unsigned_abs();
    format!("{sign}{}.{:03}", magnitude / 1_000, magnitude % 1_000)
}

/// Everything one run of the harness measured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Schema version, [`REPORT_VERSION`].
    pub schema_version: u32,
    /// The machine and build the numbers came from.
    pub host: Host,
    /// The adapter the texture stage ran on, absent when there was none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<Gpu>,
    /// One entry per fixture and scenario kind, in a fixed order.
    pub scenarios: Vec<Scenario>,
    /// What the A/V sync and drift harness measured, when it ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncSection>,
    /// What the proxy editing validation measured, when it ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxySection>,
}

impl Report {
    /// An empty report describing this machine and build.
    pub fn new(gpu: Option<Gpu>) -> Self {
        Self {
            schema_version: REPORT_VERSION,
            host: Host {
                os: std::env::consts::OS.to_owned(),
                arch: std::env::consts::ARCH.to_owned(),
                profile: if cfg!(debug_assertions) {
                    "debug".to_owned()
                } else {
                    "release".to_owned()
                },
            },
            gpu,
            scenarios: Vec::new(),
            sync: None,
            proxy: None,
        }
    }

    /// The lines printed to the log, which are what CI shows.
    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "host         {} {} ({} build)",
                self.host.os, self.host.arch, self.host.profile
            ),
            match &self.gpu {
                Some(gpu) => format!(
                    "adapter      {}{}",
                    gpu.adapter,
                    if gpu.software {
                        " [software: treat every number as an upper bound]"
                    } else {
                        ""
                    }
                ),
                None => "adapter      none: the texture stage was skipped".to_owned(),
            },
        ];
        lines.extend(self.scenarios.iter().map(Scenario::summary_line));
        if let Some(sync) = &self.sync {
            lines.extend(sync.summary_lines());
        }
        if let Some(proxy) = &self.proxy {
            lines.extend(proxy.summary_lines());
        }
        lines
    }

    /// True when at least one scenario produced numbers.
    pub fn measured_anything(&self) -> bool {
        self.scenarios
            .iter()
            .any(|scenario| scenario.status == ScenarioStatus::Measured)
            || self
                .sync
                .as_ref()
                .is_some_and(|sync| sync.status == ScenarioStatus::Measured)
            || self
                .proxy
                .as_ref()
                .is_some_and(|proxy| proxy.status == ScenarioStatus::Measured)
    }
}

/// Fills in the derived fields of a measured scenario.
pub fn finish(scenario: &mut Scenario, frames: u64, wall_nanos: u64) {
    scenario.frames = frames;
    scenario.wall_nanos = wall_nanos;
    scenario.sustained_milli_fps = rate_milli_fps(frames, wall_nanos);
}

#[cfg(test)]
mod tests {
    use super::{
        Gpu, ProxySection, REPORT_VERSION, Report, Scenario, ScenarioKind, ScenarioStatus,
        SyncSection, finish, format_mib, format_milli_frames,
    };
    use crate::stats::Samples;
    use sub_time::Rational;

    fn sync_section() -> SyncSection {
        let rate = Rational::new(25, 1).expect("25 fps");
        let mut section = SyncSection::measured("longgop_720p_10min.mp4", rate, 600);
        section.decoder = Some("avdec_h264".to_owned());
        section.seconds_played = 600;
        section.samples = 600;
        section.frames_shown = 15_000;
        section.frames_decoded = 15_000;
        section.max_offset_milli_frames = 750;
        section.worst_at_seconds = 421;
        section
    }

    fn measured() -> Scenario {
        let mut samples = Samples::new();
        for _ in 0..4 {
            samples.push(20_000_000);
        }
        let mut scenario = Scenario::skipped("bars_1080p_h264.mp4", ScenarioKind::Scrub, "unused");
        scenario.status = ScenarioStatus::Measured;
        scenario.skipped_reason = None;
        scenario.width = 1920;
        scenario.height = 1080;
        scenario.decoder = Some("avdec_h264".to_owned());
        scenario.decode = samples.summary();
        scenario.decode_to_texture = samples.summary();
        finish(&mut scenario, 4, 80_000_000);
        scenario
    }

    #[test]
    fn sustained_rate_comes_from_the_wall_clock() {
        let scenario = measured();
        // Four frames in 80 ms is 50 fps.
        assert_eq!(scenario.sustained_milli_fps, Some(50_000));
    }

    #[test]
    fn a_skipped_scenario_says_why() {
        let scenario = Scenario::skipped("bars_2160p_h264.mp4", ScenarioKind::Playback, "no file");
        let line = scenario.summary_line();
        assert!(
            line.contains("skipped: no file"),
            "unexpected line {line:?}"
        );
        assert_eq!(scenario.sustained_milli_fps, None);
    }

    #[test]
    fn a_measured_line_names_the_size_and_the_rate() {
        let line = measured().summary_line();
        assert!(line.contains("1920x1080"), "unexpected line {line:?}");
        assert!(line.contains("50.000 fps"), "unexpected line {line:?}");
        assert!(line.contains("p95"), "unexpected line {line:?}");
    }

    #[test]
    fn a_run_with_no_adapter_is_marked_decode_only() {
        let mut scenario = measured();
        scenario.upload = None;
        assert!(scenario.summary_line().contains("decode only"));
    }

    #[test]
    fn the_report_round_trips_through_json() {
        let mut report = Report::new(Some(Gpu {
            adapter: "llvmpipe (software, Vulkan)".to_owned(),
            software: true,
        }));
        report.scenarios.push(measured());
        report.scenarios.push(Scenario::skipped(
            "x.mp4",
            ScenarioKind::Playback,
            "no file",
        ));

        let json = serde_json::to_string_pretty(&report).expect("the report serialises");
        let parsed: Report = serde_json::from_str(&json).expect("the report parses back");
        assert_eq!(parsed, report);
        assert_eq!(parsed.schema_version, REPORT_VERSION);
        assert!(parsed.measured_anything());
        // Absent measurements are left out rather than written as null.
        assert!(!json.contains("null"), "unexpected null in {json}");
    }

    #[test]
    fn a_report_with_nothing_measured_says_so() {
        let mut report = Report::new(None);
        report
            .scenarios
            .push(Scenario::skipped("x.mp4", ScenarioKind::Scrub, "no file"));
        assert!(!report.measured_anything());
        let summary = report.summary_lines().join("\n");
        assert!(summary.contains("adapter      none"), "{summary}");
    }

    #[test]
    fn a_sync_run_inside_its_frame_has_not_drifted() {
        let section = sync_section();
        assert!(section.within_one_frame());
        let summary = section.summary_lines().join("\n");
        assert!(summary.contains("600 s played"), "{summary}");
        assert!(summary.contains("max drift 0.000 frames"), "{summary}");
        assert!(!summary.contains("OVER ONE FRAME"), "{summary}");
    }

    #[test]
    fn a_sync_run_a_whole_frame_out_is_flagged() {
        let mut section = sync_section();
        section.max_drift_milli_frames = 1_250;
        assert!(!section.within_one_frame());
        let summary = section.summary_lines().join("\n");
        assert!(
            summary.contains("max drift 1.250 frames at 421 s"),
            "{summary}"
        );
        assert!(summary.contains("OVER ONE FRAME"), "{summary}");
    }

    #[test]
    fn a_skipped_sync_run_says_why_and_measured_nothing() {
        let mut report = Report::new(None);
        report.sync = Some(SyncSection::skipped("longgop_720p_10min.mp4", "no file"));
        assert!(!report.measured_anything());
        let summary = report.summary_lines().join("\n");
        assert!(summary.contains("skipped: no file"), "{summary}");
    }

    #[test]
    fn a_sync_report_round_trips_through_json() {
        let mut report = Report::new(None);
        report.sync = Some(sync_section());
        assert!(report.measured_anything());
        let json = serde_json::to_string_pretty(&report).expect("the report serialises");
        let parsed: Report = serde_json::from_str(&json).expect("the report parses back");
        assert_eq!(parsed, report);
        assert!(!json.contains("null"), "unexpected null in {json}");
    }

    fn proxy_section() -> ProxySection {
        let mut section = ProxySection::skipped("longgop_1080p_1h.mp4", "unused");
        section.status = ScenarioStatus::Measured;
        section.skipped_reason = None;
        section.proxy = Some("proxy-abc-mjpeg-half-q75.mov".to_owned());
        section.proxy_codec = Some("mjpeg".to_owned());
        section.proxy_scale = Some("half".to_owned());
        section.proxy_width = 960;
        section.proxy_height = 540;
        section.proxy_frames = 90_000;
        section.source_bytes = 400 * 1024 * 1024;
        section.proxy_bytes = 900 * 1024 * 1024;
        section.generated = true;
        section.generation_nanos = 1_200_000_000_000;
        section.playback_seconds = 5;
        section.frame_interval_nanos = 40_000_000;
        section.scrub_target_milli_fps = 30_000;
        section.proxy_scrub_milli_fps = Some(42_500);
        section.source_scrub_milli_fps = Some(3_200);
        section.proxy_playback_stalls = Some(0);
        section.proxy_playback_max_nanos = Some(18_000_000);
        section.memory_budget_bytes = 2_048 * 1024 * 1024;
        section.peak_rss_bytes = Some(700 * 1024 * 1024);
        section
    }

    #[test]
    fn a_proxy_run_that_meets_every_number_passes() {
        let section = proxy_section();
        assert!(section.scrub_ok());
        assert!(section.playback_ok());
        assert!(section.memory_ok());
        assert!(section.failures().is_empty());
        let summary = section.summary_lines().join("\n");
        assert!(
            summary.contains("scrub 42.500 fps (target 30.000) [PASS]"),
            "{summary}"
        );
        assert!(
            summary.contains("playback 0 stalls over 5 s [PASS]"),
            "{summary}"
        );
        assert!(
            summary.contains("peak memory 700 MiB of a 2048 MiB budget [PASS]"),
            "{summary}"
        );
        assert!(!summary.contains("FAILED"), "{summary}");
    }

    #[test]
    fn each_missed_number_is_named_once() {
        let mut section = proxy_section();
        section.proxy_scrub_milli_fps = Some(29_999);
        section.proxy_playback_stalls = Some(3);
        section.peak_rss_bytes = Some(4_096 * 1024 * 1024);
        assert!(!section.scrub_ok());
        assert!(!section.playback_ok());
        assert!(!section.memory_ok());
        assert_eq!(section.failures().len(), 3);
        let summary = section.summary_lines().join("\n");
        assert!(summary.contains("under the 30.000 fps"), "{summary}");
        assert!(
            summary.contains("3 of the timed playback steps overran"),
            "{summary}"
        );
        assert!(summary.contains("over the 2048 MiB budget"), "{summary}");
    }

    #[test]
    fn an_unmeasurable_peak_does_not_fail_the_memory_criterion() {
        let mut section = proxy_section();
        section.peak_rss_bytes = None;
        assert!(section.memory_ok());
        assert!(section.failures().is_empty());
        assert!(
            section.summary_lines().join("\n").contains("unmeasured"),
            "the summary says the peak was not measured"
        );
    }

    #[test]
    fn a_skipped_proxy_run_says_why_and_fails_nothing() {
        let mut report = Report::new(None);
        report.proxy = Some(ProxySection::skipped("longgop_1080p_1h.mp4", "no file"));
        assert!(!report.measured_anything());
        let section = report.proxy.as_ref().expect("the section is there");
        assert!(section.failures().is_empty());
        assert!(
            report
                .summary_lines()
                .join("\n")
                .contains("skipped: no file")
        );
    }

    #[test]
    fn a_proxy_report_round_trips_through_json() {
        let mut report = Report::new(None);
        report.proxy = Some(proxy_section());
        let mut playback = measured();
        playback.stall_threshold_nanos = Some(40_000_000);
        playback.stalls = Some(0);
        report.scenarios.push(playback);
        assert!(report.measured_anything());
        let json = serde_json::to_string_pretty(&report).expect("the report serialises");
        let parsed: Report = serde_json::from_str(&json).expect("the report parses back");
        assert_eq!(parsed, report);
        assert!(!json.contains("null"), "unexpected null in {json}");
        assert!(
            report.summary_lines().join("\n").contains("0 stalls"),
            "a scenario that counted stalls says so"
        );
    }

    #[test]
    fn byte_counts_print_as_whole_mebibytes() {
        assert_eq!(format_mib(0), "0 MiB");
        assert_eq!(format_mib(1024 * 1024), "1 MiB");
        assert_eq!(format_mib(3 * 1024 * 1024 - 1), "2 MiB");
    }

    #[test]
    fn milli_frames_print_as_signed_frames() {
        assert_eq!(format_milli_frames(0), "0.000");
        assert_eq!(format_milli_frames(1_250), "1.250");
        assert_eq!(format_milli_frames(-42), "-0.042");
    }
}
