//! Analysis commands: store what an analyzer plugin found, and turn it into
//! markers (docs/PLAN.md §6.2).
//!
//! An analyzer plugin cannot touch the project — it hands its findings to the
//! host, and the host stores them with [`SetMediaAnalysis`] like every other
//! mutation, so an analysis is undoable, replayable and visible on the event
//! bus. Findings live on the [`MediaItem`](sub_model::MediaItem) rather than on
//! a clip, because they describe the file: one run is correct for every clip
//! cut from that media.
//!
//! [`MarkersFromAnalysis`] is the second half. It is deliberately a separate
//! command: an analysis is a proposal, and nothing appears on the timeline
//! until someone — a person, an agent or a plugin — asks for it. Because
//! analysis times are media times, the markers it makes go onto a *clip*,
//! whose marker list is in that same source time; no rate conversion happens,
//! and none of the arithmetic leaves [`RationalTime`](sub_time::RationalTime).
//!
//! All three storing commands invert to [`ReplaceMediaAnalyses`] carrying the
//! whole previous list, which is the same trick the rest of the command set
//! uses: undo restores what was there rather than something that looks like
//! it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_model::{Analysis, ClipId, Marker, MediaId, Project, SequenceId, TrackId};
use sub_time::TimeRange;

use super::{clip_mut, media_item_mut};
use crate::{Command, Inverse, codes};

/// Stores one analyzer's findings on a media item, replacing what that same
/// analyzer found before.
///
/// This is what the host applies when an analysis job finishes. Other
/// analyzers' findings are left alone: a media item holds at most one analysis
/// per analyzer, keyed by [`Analysis::analyzer`].
///
/// ```
/// use sub_edit::History;
/// use sub_edit::commands::{ImportMedia, SetMediaAnalysis};
/// use sub_model::{Analysis, MediaItem, MediaPath, Project};
///
/// let mut project = Project::new("Doc cut");
/// let item = MediaItem::new(MediaPath::new("footage/interview.mp4").unwrap());
/// let media = item.id;
///
/// let mut history = History::new();
/// history.apply(&mut project, ImportMedia::new(item)).unwrap();
/// history
///     .apply(
///         &mut project,
///         SetMediaAnalysis::new(media, Analysis::new("loudness")),
///     )
///     .unwrap();
/// assert!(project.media_item(media).unwrap().analysis("loudness").is_some());
///
/// history.undo(&mut project).unwrap();
/// assert!(project.media_item(media).unwrap().analyses.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetMediaAnalysis {
    /// The media item the findings describe.
    pub media: MediaId,
    /// The findings, carrying the analyzer they came from.
    pub analysis: Analysis,
}

impl SetMediaAnalysis {
    /// Stores `analysis` on `media`.
    #[must_use]
    pub fn new(media: MediaId, analysis: Analysis) -> Self {
        Self { media, analysis }
    }
}

impl Command for SetMediaAnalysis {
    const KIND: &'static str = "media.set_analysis";
    const DESCRIPTION: &'static str =
        "Store one analyzer's findings on a media item, replacing that analyzer's previous run.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let media = self.media;
        let item = media_item_mut(project, media)?;
        let inverse = ReplaceMediaAnalyses::new(media, item.analyses.clone());

        match item
            .analyses
            .iter_mut()
            .find(|held| held.analyzer == self.analysis.analyzer)
        {
            Some(held) => *held = self.analysis.clone(),
            None => item.analyses.push(self.analysis.clone()),
        }
        Ok(Inverse::new(inverse))
    }

    fn label(&self) -> String {
        format!("Store {} analysis", self.analysis.analyzer)
    }
}

/// Removes one analyzer's findings from a media item.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveMediaAnalysis {
    /// The media item to clear.
    pub media: MediaId,
    /// Which analyzer's findings to drop.
    pub analyzer: String,
}

impl RemoveMediaAnalysis {
    /// Drops what `analyzer` found about `media`.
    #[must_use]
    pub fn new(media: MediaId, analyzer: impl Into<String>) -> Self {
        Self {
            media,
            analyzer: analyzer.into(),
        }
    }
}

impl Command for RemoveMediaAnalysis {
    const KIND: &'static str = "media.remove_analysis";
    const DESCRIPTION: &'static str = "Remove one analyzer's findings from a media item.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let media = self.media;
        let item = media_item_mut(project, media)?;
        let index = item
            .analyses
            .iter()
            .position(|held| held.analyzer == self.analyzer)
            .ok_or_else(|| {
                SubError::new(
                    codes::ANALYSIS_NOT_FOUND,
                    "this media item has no findings from that analyzer",
                )
                .with_detail("media", media)
                .with_detail("analyzer", self.analyzer.clone())
            })?;

        let inverse = ReplaceMediaAnalyses::new(media, item.analyses.clone());
        item.analyses.remove(index);
        Ok(Inverse::new(inverse))
    }

    fn label(&self) -> String {
        format!("Remove {} analysis", self.analyzer)
    }
}

/// Replaces a media item's whole analysis list.
///
/// This is the inverse of both storing commands, and its own inverse, so undo
/// and redo of an analysis are exact.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceMediaAnalyses {
    /// The media item to rewrite.
    pub media: MediaId,
    /// The list it is left holding, at most one entry per analyzer.
    pub analyses: Vec<Analysis>,
}

impl ReplaceMediaAnalyses {
    /// Leaves `media` holding exactly `analyses`.
    #[must_use]
    pub fn new(media: MediaId, analyses: Vec<Analysis>) -> Self {
        Self { media, analyses }
    }
}

impl Command for ReplaceMediaAnalyses {
    const KIND: &'static str = "media.replace_analyses";
    const DESCRIPTION: &'static str = "Replace a media item's whole list of analyzer findings.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        for (index, analysis) in self.analyses.iter().enumerate() {
            if self.analyses[..index]
                .iter()
                .any(|earlier| earlier.analyzer == analysis.analyzer)
            {
                return Err(SubError::new(
                    codes::DUPLICATE_ANALYSIS,
                    "a media item holds at most one analysis per analyzer",
                )
                .with_detail("media", self.media)
                .with_detail("analyzer", analysis.analyzer.clone()));
            }
        }

        let media = self.media;
        let item = media_item_mut(project, media)?;
        let previous = std::mem::replace(&mut item.analyses, self.analyses.clone());
        Ok(Inverse::new(Self::new(media, previous)))
    }

    fn label(&self) -> String {
        "Replace media analyses".to_owned()
    }
}

/// Turns a stored analysis into markers on a clip.
///
/// The clip names the media, so the findings that apply are the ones stored on
/// the clip's own media item. Analysis times are media times and a clip's
/// markers are in that same source time, so the markers are copied across
/// unchanged: no rate conversion, no rounding, nothing to drift.
///
/// Two things are filtered out. Findings that fall outside the clip's
/// `source_range` are skipped, because the clip does not show that part of the
/// file; and a finding whose identifier is already on the clip is skipped, so
/// running the command twice is idempotent rather than a duplicate-marker
/// error. `label` narrows it further to ranges carrying one label — the way a
/// cut-silence workflow marks only the silence.
///
/// Identifiers come from the stored analysis, so replaying this command
/// reproduces the same project byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkersFromAnalysis {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip the markers land on.
    pub clip: ClipId,
    /// Which analyzer's findings to use.
    pub analyzer: String,
    /// When present, only ranges with this label are used and the analysis's
    /// own markers are left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl MarkersFromAnalysis {
    /// Marks `clip` with everything `analyzer` found in the media it uses.
    #[must_use]
    pub fn new(
        sequence: SequenceId,
        track: TrackId,
        clip: ClipId,
        analyzer: impl Into<String>,
    ) -> Self {
        Self {
            sequence,
            track,
            clip,
            analyzer: analyzer.into(),
            label: None,
        }
    }

    /// The same command, restricted to ranges labelled `label`.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The findings `analysis` contributes, as markers.
    fn selected(&self, analysis: &Analysis) -> Vec<Marker> {
        match &self.label {
            Some(label) => analysis
                .ranges_labelled(label)
                .map(sub_model::AnalysisRange::to_marker)
                .collect(),
            None => analysis.to_markers(),
        }
    }
}

/// Whether `source` shows the span a marker covers.
///
/// A point marker is visible when the clip contains the instant; a span is
/// visible when it overlaps the clip at all, and is copied whole rather than
/// clipped, so undo and a later re-run agree on what it was.
fn visible_in(source: TimeRange, marked_range: TimeRange) -> bool {
    if marked_range.is_empty() {
        source.contains(marked_range.start())
    } else {
        source.overlaps(marked_range)
    }
}

impl Command for MarkersFromAnalysis {
    const KIND: &'static str = "marker.from_analysis";
    const DESCRIPTION: &'static str =
        "Turn an analyzer's findings about a clip's media into markers on that clip.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        // Validate the clip first — this is also what rejects a locked track —
        // and take the two facts needed to pick findings before the immutable
        // look at the media item.
        let clip = clip_mut(project, self.sequence, self.track, self.clip)?;
        let (media, source) = (clip.media, clip.source_range);

        let item = project.media_item(media).ok_or_else(|| {
            SubError::new(
                codes::MEDIA_NOT_FOUND,
                "the clip references a media item the project does not hold",
            )
            .with_detail("media", media)
        })?;
        let analysis = item.analysis(&self.analyzer).ok_or_else(|| {
            SubError::new(
                codes::ANALYSIS_NOT_FOUND,
                "this media item has no findings from that analyzer",
            )
            .with_detail("media", media)
            .with_detail("analyzer", self.analyzer.clone())
        })?;
        let found = self.selected(analysis);

        let clip = clip_mut(project, self.sequence, self.track, self.clip)?;
        let previous = clip.markers.clone();
        for marker in found {
            if !visible_in(source, marker.marked_range) {
                continue;
            }
            if clip.markers.iter().any(|held| held.id == marker.id) {
                continue;
            }
            clip.markers.push(marker);
        }

        Ok(Inverse::new(ReplaceClipMarkers::new(
            self.sequence,
            self.track,
            self.clip,
            previous,
        )))
    }

    fn label(&self) -> String {
        format!("Mark {} findings", self.analyzer)
    }
}

/// Replaces a clip's whole marker list.
///
/// The inverse of [`MarkersFromAnalysis`], and its own inverse: it carries the
/// markers whole, identifiers and notes included.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceClipMarkers {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip to rewrite.
    pub clip: ClipId,
    /// The markers it is left holding, in order.
    pub markers: Vec<Marker>,
}

impl ReplaceClipMarkers {
    /// Leaves `clip` holding exactly `markers`.
    #[must_use]
    pub fn new(sequence: SequenceId, track: TrackId, clip: ClipId, markers: Vec<Marker>) -> Self {
        Self {
            sequence,
            track,
            clip,
            markers,
        }
    }
}

impl Command for ReplaceClipMarkers {
    const KIND: &'static str = "marker.replace_on_clip";
    const DESCRIPTION: &'static str = "Replace a clip's whole marker list.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        for (index, marker) in self.markers.iter().enumerate() {
            if self.markers[..index]
                .iter()
                .any(|earlier| earlier.id == marker.id)
            {
                return Err(SubError::new(
                    codes::DUPLICATE_MARKER,
                    "a marker with this identifier is already on the target",
                )
                .with_detail("marker_id", marker.id));
            }
        }

        let clip = clip_mut(project, self.sequence, self.track, self.clip)?;
        let previous = std::mem::replace(&mut clip.markers, self.markers.clone());
        Ok(Inverse::new(Self::new(
            self.sequence,
            self.track,
            self.clip,
            previous,
        )))
    }

    fn label(&self) -> String {
        "Replace clip markers".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use sub_model::{
        AnalysisRange, Clip, ColorTags, MediaItem, MediaPath, Resolution, Sequence,
        SequenceSettings, Track, TrackItem, TrackKind,
    };
    use sub_time::{Rational, RationalTime};

    fn rate() -> Rational {
        Rational::FPS_23_976
    }

    fn span(start: i64, duration: i64) -> TimeRange {
        TimeRange::new(
            RationalTime::from_frames(start, rate()),
            RationalTime::from_frames(duration, rate()),
        )
        .unwrap()
    }

    struct Fixture {
        project: Project,
        media: MediaId,
        sequence: SequenceId,
        track: TrackId,
        clip: ClipId,
    }

    /// A project with one clip using frames 0..48 of a media item.
    fn fixture() -> Fixture {
        let settings =
            SequenceSettings::new(Resolution::HD_1080, rate(), 48_000, ColorTags::default())
                .unwrap();
        let mut sequence = Sequence::new("Main", settings);
        let item = MediaItem::new(MediaPath::new("footage/interview.mp4").unwrap());
        let media = item.id;
        let mut track = Track::new("V1", TrackKind::Video);
        let clip = Clip::new("A", media, span(0, 48));
        let clip_id = clip.id;
        track.items.push(TrackItem::Clip(clip));
        let track_id = track.id;
        sequence.tracks.push(track);
        let sequence_id = sequence.id;

        let mut project = Project::new("Doc cut");
        project.media.push(item);
        project.sequences.push(sequence);

        Fixture {
            project,
            media,
            sequence: sequence_id,
            track: track_id,
            clip: clip_id,
        }
    }

    /// The analysis the fixture stores: one in-range span, one out of range.
    fn silence() -> Analysis {
        Analysis::new("silence").with_ranges(vec![
            AnalysisRange::new("silence", span(12, 6)),
            AnalysisRange::new("speech", span(20, 4)),
            AnalysisRange::new("silence", span(100, 6)),
        ])
    }

    #[test]
    fn storing_an_analysis_replaces_only_that_analyzers_run() {
        let mut fx = fixture();
        let mut history = History::new();
        history
            .apply(
                &mut fx.project,
                SetMediaAnalysis::new(fx.media, Analysis::new("loudness")),
            )
            .unwrap();
        history
            .apply(&mut fx.project, SetMediaAnalysis::new(fx.media, silence()))
            .unwrap();
        history
            .apply(
                &mut fx.project,
                SetMediaAnalysis::new(
                    fx.media,
                    Analysis::new("silence")
                        .with_ranges(vec![AnalysisRange::new("silence", span(0, 2))]),
                ),
            )
            .unwrap();

        let item = fx.project.media_item(fx.media).unwrap();
        assert_eq!(item.analyses.len(), 2, "the second run replaced the first");
        assert_eq!(item.analysis("silence").unwrap().ranges.len(), 1);
        assert!(item.analysis("loudness").is_some());

        history.undo(&mut fx.project).unwrap();
        let item = fx.project.media_item(fx.media).unwrap();
        assert_eq!(item.analysis("silence").unwrap().ranges.len(), 3);
    }

    #[test]
    fn removing_an_analysis_that_never_ran_is_an_error() {
        let mut fx = fixture();
        let mut history = History::new();
        let err = history
            .apply(
                &mut fx.project,
                RemoveMediaAnalysis::new(fx.media, "silence"),
            )
            .unwrap_err();
        assert_eq!(err.code, codes::ANALYSIS_NOT_FOUND);
    }

    #[test]
    fn removing_an_analysis_puts_it_back_on_undo() {
        let mut fx = fixture();
        let analysis = silence();
        let mut history = History::new();
        history
            .apply(
                &mut fx.project,
                SetMediaAnalysis::new(fx.media, analysis.clone()),
            )
            .unwrap();
        history
            .apply(
                &mut fx.project,
                RemoveMediaAnalysis::new(fx.media, "silence"),
            )
            .unwrap();
        assert!(fx.project.media_item(fx.media).unwrap().analyses.is_empty());

        history.undo(&mut fx.project).unwrap();
        assert_eq!(
            fx.project.media_item(fx.media).unwrap().analysis("silence"),
            Some(&analysis)
        );
    }

    #[test]
    fn two_analyses_from_one_analyzer_are_refused() {
        let mut fx = fixture();
        let mut history = History::new();
        let err = history
            .apply(
                &mut fx.project,
                ReplaceMediaAnalyses::new(fx.media, vec![silence(), silence()]),
            )
            .unwrap_err();
        assert_eq!(err.code, codes::DUPLICATE_ANALYSIS);
        assert!(fx.project.media_item(fx.media).unwrap().analyses.is_empty());
    }

    #[test]
    fn findings_become_clip_markers_in_source_time() {
        let mut fx = fixture();
        let analysis = silence();
        let mut history = History::new();
        history
            .apply(
                &mut fx.project,
                SetMediaAnalysis::new(fx.media, analysis.clone()),
            )
            .unwrap();
        history
            .apply(
                &mut fx.project,
                MarkersFromAnalysis::new(fx.sequence, fx.track, fx.clip, "silence"),
            )
            .unwrap();

        let markers = &clip_of(&fx).markers;
        assert_eq!(markers.len(), 2, "the finding past the clip end is skipped");
        assert_eq!(markers[0].id, analysis.ranges[0].id);
        assert_eq!(markers[0].marked_range, span(12, 6));
        assert_eq!(markers[1].name, "speech");

        history.undo(&mut fx.project).unwrap();
        assert!(clip_of(&fx).markers.is_empty());
    }

    #[test]
    fn a_label_narrows_the_findings_and_re_running_adds_nothing() {
        let mut fx = fixture();
        let mut history = History::new();
        history
            .apply(&mut fx.project, SetMediaAnalysis::new(fx.media, silence()))
            .unwrap();
        let command = MarkersFromAnalysis::new(fx.sequence, fx.track, fx.clip, "silence")
            .with_label("silence");
        history.apply(&mut fx.project, command.clone()).unwrap();
        assert_eq!(clip_of(&fx).markers.len(), 1);

        history.apply(&mut fx.project, command).unwrap();
        assert_eq!(clip_of(&fx).markers.len(), 1, "the command is idempotent");
    }

    #[test]
    fn marking_from_an_analysis_that_never_ran_is_an_error() {
        let mut fx = fixture();
        let mut history = History::new();
        let err = history
            .apply(
                &mut fx.project,
                MarkersFromAnalysis::new(fx.sequence, fx.track, fx.clip, "silence"),
            )
            .unwrap_err();
        assert_eq!(err.code, codes::ANALYSIS_NOT_FOUND);
    }

    #[test]
    fn marking_a_locked_track_is_refused() {
        let mut fx = fixture();
        let mut history = History::new();
        history
            .apply(&mut fx.project, SetMediaAnalysis::new(fx.media, silence()))
            .unwrap();
        fx.project.sequences[0].tracks[0].locked = true;
        let err = history
            .apply(
                &mut fx.project,
                MarkersFromAnalysis::new(fx.sequence, fx.track, fx.clip, "silence"),
            )
            .unwrap_err();
        assert_eq!(err.code, codes::TRACK_LOCKED);
    }

    /// The fixture's one clip.
    fn clip_of(fx: &Fixture) -> &Clip {
        match &fx.project.sequences[0].tracks[0].items[0] {
            TrackItem::Clip(clip) => clip,
            other => panic!("the fixture holds one clip, found {other:?}"),
        }
    }
}
