//! Cut silence: the first-party plugin that finds the gaps and takes them out.
//!
//! This is the worked example the plugin system exists for (docs/PLAN.md §6):
//! an agent-sized plugin that does something a person actually asks for, in two
//! halves that mirror the architecture's own split between *looking* and
//! *editing*.
//!
//! - **`analyze`** (the `analyzer` world) reads one media item, measures it,
//!   and reports the silent spans in media time. It changes nothing. The host
//!   runs it as a background job and stores what it found on the media item
//!   with an ordinary undoable command, so an analysis can be reviewed, re-run
//!   and undone before anything moves on the timeline.
//! - **`run`** (the `command` world) reads those findings back, works out what
//!   they mean for the clips the caller names, and removes them: one
//!   `clip.split` at each edge and one `clip.ripple_delete` for the middle,
//!   through the Command API, inside a single `edit.begin_group` /
//!   `edit.commit_group` pair. However many primitives a cut takes, the user
//!   undoes the whole thing in one step, and the GUI, the CLI and the MCP
//!   bridge all see exactly the same commands.
//!
//! Both live in one component because a plugin is one component; `wit/` explains
//! the world that makes that possible, and `CLAUDE.md` is the guide for anyone —
//! human or agent — writing the next one.
//!
//! Everything worth testing is out of this file: [`wav`] reads the sound,
//! [`silence`] decides what is silent, and [`plan`] does the timeline
//! arithmetic. Those three are plain Rust with no WIT in them, and they carry
//! the tests. What is left here is the glue: JSON in, Command API calls out.

#![allow(missing_docs, clippy::all, clippy::pedantic)]

wit_bindgen::generate!({
    path: ["../../wit", "wit"],
    world: "subordinate:cut-silence/cut-silence@0.1.0",
    generate_all,
});

pub mod plan;
pub mod silence;
pub mod wav;

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::plan::{Cut, Range, Rate};
use crate::silence::Options;
use crate::subordinate::plugin::analysis::{AnalysisMarker, AnalysisRange};
use crate::subordinate::plugin::command_api::{
    self, ClipMetadata, LogLevel, SequenceMetadata, TrackMetadata,
};
use crate::subordinate::plugin::types::{
    Detail, Rational, RationalTime, SequenceId, TimeRange, TrackId,
};

/// The plugin's identity, as `plugin.toml` declares it. The host files an
/// analysis under the id of the plugin that produced it, so this is also the
/// default name `run` looks findings up by.
pub const PLUGIN_ID: &str = "com.subordinate.cut-silence";

/// The label the analyzer puts on a silent span, and the one the command
/// matches on. A small stable vocabulary is the contract between the two
/// halves; renaming this breaks every analysis already stored in a project.
pub const LABEL: &str = "silence";

/// Where the host mounts the `$PROJECT` capability inside the sandbox.
const PROJECT_MOUNT: &str = "/project";

/// The label the undo menu shows for one run of the command.
const UNDO_LABEL: &str = "Cut Silence";

// ---------------------------------------------------------------- the exports

struct CutSilence;

impl Guest for CutSilence {
    fn analyze(media: MediaId, options: String) -> Result<AnalysisResult, Error> {
        let options = parse_options(&options)?;
        let bytes = match read_media(&media) {
            Ok(bytes) => bytes,
            Err(reason) => return Ok(unread(&reason)),
        };
        let wav = match wav::Wav::parse(&bytes) {
            Ok(wav) => wav,
            Err(err) => return Ok(unread(&err.message())),
        };

        let settings = options.at_rate(wav.sample_rate);
        let spans = silence::detect(wav.frames(), &settings);
        let rate = Rate::per_second(wav.sample_rate);
        let silent_frames: u64 = spans.iter().map(|span| span.len()).sum();

        Ok(AnalysisResult {
            markers: spans
                .iter()
                .map(|span| AnalysisMarker {
                    name: LABEL.to_owned(),
                    note: String::new(),
                    marked_range: wit_range(span.start, span.len(), rate),
                })
                .collect(),
            ranges: spans
                .iter()
                .map(|span| AnalysisRange {
                    label: LABEL.to_owned(),
                    range: wit_range(span.start, span.len(), rate),
                })
                .collect(),
            metadata: vec![
                detail("status", json!("analysed")),
                detail("threshold_db", json!(options.threshold_db)),
                detail("minimum_seconds", json!(options.minimum_seconds)),
                detail("padding_seconds", json!(options.padding_seconds)),
                detail("window_seconds", json!(options.window_seconds)),
                detail("sample_rate", json!(wav.sample_rate)),
                detail("channels", json!(wav.channels)),
                detail("frames", json!(wav.frame_count())),
                detail("silent_frames", json!(silent_frames)),
            ],
        })
    }

    fn run(project: ProjectId, args: String) -> Result<String, Error> {
        let args = Arguments::parse(&args)?;
        let sequence = choose_sequence(&project, args.sequence.as_deref())?;
        let rate = rate_of(&sequence.frame_rate)?;
        let silence = stored_silence(&project, &args)?;
        let (clips, skipped) = timeline_clips(&project, &sequence, &args)?;
        let cuts = plan::plan(&clips, &silence, rate);

        let removed: i64 = cuts.iter().map(Cut::len).sum();
        let mut report = json!({
            "sequence": sequence.id.value,
            "analyzer": args.analyzer,
            "label": args.label,
            "cuts": cuts.len(),
            "applied": 0,
            "removed": time_value(removed, rate),
            "locked_tracks_skipped": skipped,
            "dry_run": args.dry_run,
        });

        if cuts.is_empty() || args.dry_run {
            command_api::log(
                LogLevel::Info,
                &format!("{PLUGIN_ID}: {} cuts planned, none applied", cuts.len()),
            );
            return Ok(report.to_string());
        }

        // One group around the whole cut: every split and every ripple delete
        // below is an ordinary command, and the user undoes all of them in one
        // step. A failure part-way through aborts the group, which reverses
        // what has already been applied, so a cut is all or nothing.
        //
        // The host opens a group around a plugin command run of its own accord;
        // groups do not nest, so this opens one only when it finds none open,
        // and closes only the group it opened.
        let ours = !in_group(&project)?;
        if ours {
            call(&project, "edit.begin_group", json!({ "label": UNDO_LABEL }))?;
        }
        let mut applied = 0_usize;
        for cut in &cuts {
            if let Err(failure) = apply_cut(&project, &sequence.id.value, cut, rate) {
                let hint = if ours {
                    let _ = call(&project, "edit.abort_group", json!({}));
                    "the group was aborted, so the project is as it was before the run"
                } else {
                    "the host's own group is still open; aborting it undoes the whole run"
                };
                return Err(failure
                    .detail("cuts_planned", &cuts.len().to_string())
                    .detail("cuts_applied", &applied.to_string())
                    .detail("hint", hint));
            }
            applied += 1;
        }
        if ours {
            call(&project, "edit.commit_group", json!({}))?;
        }

        command_api::log(
            LogLevel::Info,
            &format!("{PLUGIN_ID}: applied {applied} cuts as one undo step"),
        );
        report["applied"] = json!(applied);
        report["group"] = json!(if ours { "plugin" } else { "host" });
        Ok(report.to_string())
    }
}

export!(CutSilence);

// ------------------------------------------------------------------- analysis

/// Reads the analyzer's options, rejecting a key it does not know rather than
/// silently ignoring a misspelt one.
fn parse_options(text: &str) -> Result<Options, Error> {
    let object = object(text, "options")?;
    let mut options = Options::default();
    for (key, value) in &object {
        match key.as_str() {
            "threshold_db" => options.threshold_db = number(value, key)? as f32,
            "window_seconds" => options.window_seconds = number(value, key)? as f32,
            "minimum_seconds" => options.minimum_seconds = number(value, key)? as f32,
            "padding_seconds" => options.padding_seconds = number(value, key)? as f32,
            other => {
                return Err(unknown_key(
                    other,
                    "threshold_db, window_seconds, minimum_seconds, padding_seconds",
                ));
            }
        }
    }
    Ok(options)
}

/// The bytes of `media`, or a sentence saying why they could not be had.
///
/// An analyzer is handed an identifier, never a path: this is the walk from one
/// to the other, through the host — the only party that knows where a project's
/// media sits — and then through the `$PROJECT` root the user granted.
fn read_media(media: &MediaId) -> Result<Vec<u8>, String> {
    let mut path = None;
    for project in command_api::open_projects() {
        let Ok(answer) = call(&project, "project.get", json!({})) else {
            continue;
        };
        for item in media_items(&answer) {
            if item.get("id").and_then(Value::as_str) == Some(media.value.as_str()) {
                path = item.get("path").and_then(Value::as_str).map(str::to_owned);
            }
        }
    }
    let Some(path) = path else {
        return Err(format!(
            "no open project holds the media item {}",
            media.value
        ));
    };

    let file = format!("{PROJECT_MOUNT}/{path}");
    command_api::log(LogLevel::Info, &format!("{PLUGIN_ID}: reading {file}"));
    std::fs::read(&file).map_err(|err| {
        format!(
            "{file} could not be read ({err}); the plugin needs fs_read on $PROJECT and the media \
             must be online"
        )
    })
}

/// The analysis a run that could not look at the media leaves behind.
///
/// Not an error: an analysis is a finding, and "there was nothing here I could
/// read" is one the host can store on the media item, show in the panel and
/// hand to an agent. Failing the job instead would leave nothing behind but a
/// log line — and it is the ordinary answer inside a sandbox that was granted
/// no filesystem, which is exactly what `subordinate-cli plugin test` is.
fn unread(reason: &str) -> AnalysisResult {
    command_api::log(LogLevel::Warn, &format!("{PLUGIN_ID}: {reason}"));
    AnalysisResult {
        markers: Vec::new(),
        ranges: Vec::new(),
        metadata: vec![
            detail("status", json!("unread")),
            detail("reason", json!(reason)),
        ],
    }
}

/// The media items of a `project.get` answer.
///
/// The answer is `{ revision, project }`, and `project` is a project *file*:
/// `{ schema_version, project }` again. Descending by name rather than by a
/// fixed depth means a wrapper added or removed around the document does not
/// silently leave this reading nothing.
fn media_items(answer: &Value) -> &[Value] {
    let mut node = answer;
    for _ in 0..4 {
        if node.get("media").is_some() {
            break;
        }
        match node.get("project") {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node.get("media")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

// -------------------------------------------------------------------- the cut

/// What one invocation of the command was asked to do.
#[derive(Debug)]
struct Arguments {
    /// The sequence to cut; the first one when absent.
    sequence: Option<String>,
    /// The clips to cut; every clip of the sequence when absent. This is what
    /// "the selected clips" means to a plugin: the host owns the selection and
    /// passes it in, because a plugin holds no editor state.
    clips: Option<Vec<String>>,
    /// The analyzer whose findings to use.
    analyzer: String,
    /// The range label to cut.
    label: String,
    /// Plan the cut and report it without applying anything.
    dry_run: bool,
}

impl Arguments {
    /// Reads the argument object, rejecting unknown keys.
    fn parse(text: &str) -> Result<Self, Error> {
        let object = object(text, "args")?;
        let mut args = Self {
            sequence: None,
            clips: None,
            analyzer: PLUGIN_ID.to_owned(),
            label: LABEL.to_owned(),
            dry_run: false,
        };
        for (key, value) in &object {
            match key.as_str() {
                "sequence" => args.sequence = Some(string(value, key)?),
                "analyzer" => args.analyzer = string(value, key)?,
                "label" => args.label = string(value, key)?,
                "dry_run" => {
                    args.dry_run = value.as_bool().ok_or_else(|| {
                        error("core.invalid_argument", format!("{key} must be a boolean"))
                    })?;
                }
                "clips" => {
                    let list = value.as_array().ok_or_else(|| {
                        error(
                            "core.invalid_argument",
                            "clips must be an array of clip identifiers",
                        )
                    })?;
                    args.clips = Some(
                        list.iter()
                            .map(|clip| string(clip, "clips"))
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                }
                other => {
                    return Err(unknown_key(
                        other,
                        "sequence, clips, analyzer, label, dry_run",
                    ));
                }
            }
        }
        Ok(args)
    }
}

/// Whether a command group is already open on the project's history.
fn in_group(project: &ProjectId) -> Result<bool, Error> {
    let history = call(project, "history.get", json!({}))?;
    Ok(history
        .get("in_group")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

/// The sequence the command acts on: the one named, or the first.
fn choose_sequence(project: &ProjectId, wanted: Option<&str>) -> Result<SequenceMetadata, Error> {
    let sequences = command_api::sequences(project)?;
    match wanted {
        Some(id) => sequences
            .into_iter()
            .find(|sequence| sequence.id.value == id)
            .ok_or_else(|| {
                error("core.not_found", "the project has no such sequence").detail("sequence", id)
            }),
        None => sequences.into_iter().next().ok_or_else(|| {
            error("core.not_found", "the project has no sequences to cut")
                .detail("hint", "create a sequence before cutting silence")
        }),
    }
}

/// The silent spans every media item of the project carries, from the analysis
/// the analyzer half stored.
fn stored_silence(
    project: &ProjectId,
    args: &Arguments,
) -> Result<BTreeMap<String, Vec<Range>>, Error> {
    let answer = call(project, "project.get", json!({}))?;
    let mut silence: BTreeMap<String, Vec<Range>> = BTreeMap::new();
    for item in media_items(&answer) {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let analyses = item
            .get("analyses")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for analysis in analyses {
            if analysis.get("analyzer").and_then(Value::as_str) != Some(args.analyzer.as_str()) {
                continue;
            }
            let ranges = analysis
                .get("ranges")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            for range in ranges {
                if range.get("label").and_then(Value::as_str) != Some(args.label.as_str()) {
                    continue;
                }
                if let Some(span) = json_range(range.get("range")) {
                    silence.entry(id.to_owned()).or_default().push(span);
                }
            }
        }
    }
    Ok(silence)
}

/// Every clip the command may cut, and how many tracks were skipped because
/// they are locked.
///
/// A locked track rejects edits, and a plugin that asked anyway would abort the
/// whole group over a track the user deliberately protected. Skipping it and
/// saying so in the report is the honest answer.
fn timeline_clips(
    project: &ProjectId,
    sequence: &SequenceMetadata,
    args: &Arguments,
) -> Result<(Vec<plan::Clip>, usize), Error> {
    let rate = rate_of(&sequence.frame_rate)?;
    let tracks: Vec<TrackMetadata> = command_api::tracks(project, &sequence.id)?;
    let mut clips = Vec::new();
    let mut skipped = 0;
    for track in &tracks {
        if track.locked {
            skipped += 1;
            continue;
        }
        for clip in command_api::clips(project, &sequence.id, &track.id)? {
            if let Some(wanted) = &args.clips
                && !wanted.contains(&clip.id.value)
            {
                continue;
            }
            if let Some(clip) = timeline_clip(&track.id.value, &clip, rate) {
                clips.push(clip);
            }
        }
    }
    Ok((clips, skipped))
}

/// One clip's metadata as the planner wants it, or `None` when either of its
/// spans carries a rate the host should never have handed out.
///
/// The timeline span is brought to the sequence's own rate. The host counts a
/// clip's placement in whatever rate the arithmetic that produced it landed on
/// — a clip after a split can be placed in the media's sample rate rather than
/// the sequence's frame rate — so comparing two placements, or a placement with
/// a planned cut, means putting them at one rate first.
fn timeline_clip(track: &str, clip: &ClipMetadata, rate: Rate) -> Option<plan::Clip> {
    Some(plan::Clip {
        id: clip.id.value.clone(),
        track: track.to_owned(),
        media: clip.media.value.clone(),
        timeline: timeline_span(clip, rate)?,
        source: wit_to_range(&clip.source_range)?,
    })
}

/// Where a clip sits, in ticks of `rate`.
fn timeline_span(clip: &ClipMetadata, rate: Rate) -> Option<Range> {
    let start = plan::rescale_floor(
        clip.timeline_range.start.value,
        rate_of(&clip.timeline_range.start.rate).ok()?,
        rate,
    );
    let duration = plan::rescale_floor(
        clip.timeline_range.duration.value,
        rate_of(&clip.timeline_range.duration.rate).ok()?,
        rate,
    );
    Some(Range::new(start, duration, rate))
}

/// Applies one cut: split off the head, split off the tail, ripple away what is
/// left in the middle.
///
/// The project is looked at again between the steps rather than reasoned about,
/// because a split gives the tail a fresh identity the plugin never chose.
/// Positions are what stay true: after a split at `cut.start`, the clip that
/// begins exactly there is the one to remove.
fn apply_cut(project: &ProjectId, sequence: &str, cut: &Cut, rate: Rate) -> Result<(), Error> {
    let sequence_id = SequenceId {
        value: sequence.to_owned(),
    };
    let track_id = TrackId {
        value: cut.track.clone(),
    };

    let (clip, span) = clip_at(project, &sequence_id, &track_id, cut.start, rate)?;
    if cut.start > span.start {
        call(
            project,
            "clip.split",
            json!({
                "sequence": sequence,
                "track": cut.track,
                "clip": clip.id.value,
                "at": time_value(cut.start, rate),
            }),
        )?;
    }

    let (clip, span) = clip_at(project, &sequence_id, &track_id, cut.start, rate)?;
    if cut.end < span.end {
        call(
            project,
            "clip.split",
            json!({
                "sequence": sequence,
                "track": cut.track,
                "clip": clip.id.value,
                "at": time_value(cut.end, rate),
            }),
        )?;
    }

    // The head of a split keeps the clip's identity, so this is still the right
    // clip: it now spans exactly the silence.
    call(
        project,
        "clip.ripple_delete",
        json!({
            "sequence": sequence,
            "track": cut.track,
            "clip": clip.id.value,
        }),
    )?;
    Ok(())
}

/// The clip covering `tick` on `track`, read from the project as it is now,
/// with its placement in ticks of `rate`.
fn clip_at(
    project: &ProjectId,
    sequence: &SequenceId,
    track: &TrackId,
    tick: i64,
    rate: Rate,
) -> Result<(ClipMetadata, Range), Error> {
    command_api::clips(project, sequence, track)?
        .into_iter()
        .find_map(|clip| {
            let span = timeline_span(&clip, rate)?;
            (span.start <= tick && tick < span.end).then_some((clip, span))
        })
        .ok_or_else(|| {
            error(
                "core.not_found",
                "no clip covers the span the plan named; the project moved under the run",
            )
            .detail("track", &track.value)
            .detail("at", &tick.to_string())
        })
}

// ------------------------------------------------------------ the Command API

/// One Command API call, with the answer parsed.
fn call(project: &ProjectId, method: &str, params: Value) -> Result<Value, Error> {
    let answer = command_api::run_command(project, method, &params.to_string())?;
    serde_json::from_str(&answer).map_err(|err| {
        error(
            "core.internal",
            format!("the host's answer to {method} was not JSON: {err}"),
        )
    })
}

// ------------------------------------------------------------------ JSON glue

/// Parses a JSON object argument, which `{}` and the empty string both satisfy.
fn object(text: &str, what: &str) -> Result<Map<String, Value>, Error> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(error(
            "core.invalid_argument",
            format!("{what} must be a JSON object"),
        )),
        Err(err) => Err(error(
            "core.invalid_argument",
            format!("{what} is not JSON: {err}"),
        )),
    }
}

/// Reads a number, rejecting anything else.
fn number(value: &Value, key: &str) -> Result<f64, Error> {
    value.as_f64().ok_or_else(|| {
        error(
            "core.invalid_argument",
            format!("{key} must be a number of seconds or decibels"),
        )
    })
}

/// Reads a string, rejecting anything else.
fn string(value: &Value, key: &str) -> Result<String, Error> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| error("core.invalid_argument", format!("{key} must be a string")))
}

/// The failure a key nobody knows about deserves.
fn unknown_key(key: &str, known: &str) -> Error {
    error("core.invalid_argument", format!("unknown key {key}"))
        .detail("key", key)
        .detail("known", known)
}

/// A `rational-time` in the JSON the Command API's own types use.
fn time_value(ticks: i64, rate: Rate) -> Value {
    json!({
        "value": ticks,
        "rate": { "numerator": rate.numerator, "denominator": rate.denominator },
    })
}

/// One media-time span out of a stored analysis.
fn json_range(range: Option<&Value>) -> Option<Range> {
    let (start, rate) = json_time(range?.get("start"))?;
    let (length, length_rate) = json_time(range?.get("duration"))?;
    let length = if length_rate == rate {
        length
    } else {
        plan::rescale_floor(length, length_rate, rate)
    };
    Some(Range::new(start, length, rate))
}

/// One instant out of a stored analysis: a tick count and its rate.
fn json_time(time: Option<&Value>) -> Option<(i64, Rate)> {
    let time = time?;
    let value = time.get("value")?.as_i64()?;
    let rate = time.get("rate")?;
    let numerator = u32::try_from(rate.get("numerator")?.as_u64()?).ok()?;
    let denominator = u32::try_from(rate.get("denominator")?.as_u64()?).ok()?;
    (numerator != 0 && denominator != 0).then_some((
        value,
        Rate {
            numerator,
            denominator,
        },
    ))
}

// ------------------------------------------------------------------- WIT glue

/// A `rational-time` at `rate`.
fn wit_time(ticks: u64, rate: Rate) -> RationalTime {
    RationalTime {
        value: i64::try_from(ticks).unwrap_or(i64::MAX),
        rate: Rational {
            numerator: rate.numerator,
            denominator: rate.denominator,
        },
    }
}

/// A `time-range` starting at `start` and lasting `length` ticks of `rate`.
fn wit_range(start: u64, length: u64, rate: Rate) -> TimeRange {
    TimeRange {
        start: wit_time(start, rate),
        duration: wit_time(length, rate),
    }
}

/// A host `time-range` as the planner's half-open span.
fn wit_to_range(range: &TimeRange) -> Option<Range> {
    let rate = rate_of(&range.start.rate).ok()?;
    Some(Range::new(
        range.start.value,
        rescaled_duration(range)?,
        rate,
    ))
}

/// A range's length in ticks of its *start's* rate.
///
/// The two halves of a `time-range` carry a rate each. They are the same rate
/// in everything the host hands out; when they are not, the length is rounded
/// down, which shortens the span rather than lengthening it, so a cut derived
/// from it stays inside what it was meant to cover.
fn rescaled_duration(range: &TimeRange) -> Option<i64> {
    let start = rate_of(&range.start.rate).ok()?;
    let duration = rate_of(&range.duration.rate).ok()?;
    Some(if start == duration {
        range.duration.value
    } else {
        plan::rescale_floor(range.duration.value, duration, start)
    })
}

/// A host `rational` as the planner's rate, refusing a zero either side.
fn rate_of(rational: &Rational) -> Result<Rate, Error> {
    if rational.numerator == 0 || rational.denominator == 0 {
        return Err(error(
            "core.invalid_state",
            "the host reported a rate with a zero in it",
        ));
    }
    Ok(Rate {
        numerator: rational.numerator,
        denominator: rational.denominator,
    })
}

/// One entry of an analysis's metadata map; the value is a JSON fragment.
fn detail(key: &str, value: Value) -> Detail {
    Detail {
        key: key.to_owned(),
        value: value.to_string(),
    }
}

/// A failure with a stable code, in the shape the host's `SubError` uses.
fn error(code: &str, message: impl Into<String>) -> Error {
    Error {
        code: code.to_owned(),
        message: message.into(),
        details: Vec::new(),
    }
}

/// Adding context to a failure, one detail at a time.
trait WithDetail {
    /// The same failure, carrying `key`.
    fn detail(self, key: &str, value: &str) -> Self;
}

impl WithDetail for Error {
    fn detail(mut self, key: &str, value: &str) -> Self {
        self.details.push(Detail {
            key: key.to_owned(),
            value: Value::String(value.to_owned()).to_string(),
        });
        self
    }
}
