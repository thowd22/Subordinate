//! The JSON shape the harness writes and the summary it prints.
//!
//! The document is stable: fields are only added, never renamed or given a new
//! meaning, so a later run can be compared against a recorded baseline in
//! `docs/PERFORMANCE.md`. Every duration is an exact nanosecond count and
//! every rate is in milli-frames per second (see [`crate::stats`]).

use serde::{Deserialize, Serialize};

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
            "{:<10} {:<24} {}x{:<5} {:>4} frames  {:>8} fps  p50 {:>10}  p95 {:>10}{}",
            self.kind.as_str(),
            self.fixture,
            self.width,
            self.height,
            self.frames,
            format_milli_fps(self.sustained_milli_fps),
            p50,
            p95,
            if self.upload.is_none() {
                "  [decode only: no GPU]"
            } else {
                ""
            }
        )
    }
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
        lines
    }

    /// True when at least one scenario produced numbers.
    pub fn measured_anything(&self) -> bool {
        self.scenarios
            .iter()
            .any(|scenario| scenario.status == ScenarioStatus::Measured)
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
    use super::{Gpu, REPORT_VERSION, Report, Scenario, ScenarioKind, ScenarioStatus, finish};
    use crate::stats::Samples;

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
}
