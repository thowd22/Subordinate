//! The planner: which spans of which clips the command removes.
//!
//! The analyzer reports silence in *media* time — the timebase of the file
//! itself. A clip uses some window of that file and sits somewhere on a track
//! whose sequence has a timebase of its own. Turning one into the other is the
//! whole of this module, and it is done in exact integer arithmetic: a tick
//! count is rescaled by multiplying by the two rates as fractions in `i128`,
//! never by going through seconds as a float (docs/PLAN.md §2, CLAUDE.md).
//!
//! Two rounding rules make the result safe rather than merely close:
//!
//! - a cut's start is rounded *up* to the next sequence tick and its end
//!   rounded *down*, so a cut never eats a tick of audible material it only
//!   partly covers;
//! - a cut is clamped to the clip that carries it, so a silence that runs past
//!   the end of the clip's source window cuts only what the clip actually
//!   plays.
//!
//! Nothing here talks to the host. It is a pure function over the metadata the
//! Command API already hands a plugin, which is what makes the interesting part
//! of a cut testable on the host triple.

use std::collections::BTreeMap;

/// An exact ratio: ticks per second, unreduced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    /// The numerator; never zero.
    pub numerator: u32,
    /// The denominator; never zero.
    pub denominator: u32,
}

impl Rate {
    /// A whole number of ticks per second, such as a sample rate.
    pub fn per_second(ticks: u32) -> Self {
        Self {
            numerator: ticks,
            denominator: 1,
        }
    }
}

/// A half-open span `[start, end)` of ticks at one rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    /// The first tick in the span.
    pub start: i64,
    /// The first tick after the span.
    pub end: i64,
    /// The rate both are counted at.
    pub rate: Rate,
}

impl Range {
    /// A span of `duration` ticks beginning at `start`.
    pub fn new(start: i64, duration: i64, rate: Rate) -> Self {
        Self {
            start,
            end: start.saturating_add(duration),
            rate,
        }
    }
}

/// One clip the command may cut, as the Command API describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    /// The clip's identifier.
    pub id: String,
    /// The track it sits on.
    pub track: String,
    /// The media item it plays.
    pub media: String,
    /// Where it sits in sequence time.
    pub timeline: Range,
    /// What it plays, in media time.
    pub source: Range,
}

/// One span to be removed, in sequence time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cut {
    /// The track holding the clip.
    pub track: String,
    /// The clip the span falls in, as the project holds it *now*: applying an
    /// earlier cut on the same track can split it, so the caller looks the
    /// clip up again by position rather than trusting this after a mutation.
    pub clip: String,
    /// The first sequence tick removed.
    pub start: i64,
    /// The first sequence tick kept after the cut.
    pub end: i64,
}

impl Cut {
    /// How many sequence ticks the cut removes.
    pub fn len(&self) -> i64 {
        self.end.saturating_sub(self.start)
    }
}

/// Rescales `ticks` from one rate to another, rounding towards negative
/// infinity.
///
/// `ticks` at `from` is `ticks * from.denominator / from.numerator` seconds,
/// which at `to` is that times `to.numerator / to.denominator`. Both rates are
/// small integers and `ticks` is an `i64`, so the product fits `i128` with room
/// to spare and the division is the only place a value is lost.
pub fn rescale_floor(ticks: i64, from: Rate, to: Rate) -> i64 {
    let numerator = i128::from(ticks) * i128::from(from.denominator) * i128::from(to.numerator);
    let denominator = i128::from(from.numerator) * i128::from(to.denominator);
    clamp(numerator.div_euclid(denominator))
}

/// Rescales `ticks`, rounding towards positive infinity.
pub fn rescale_ceil(ticks: i64, from: Rate, to: Rate) -> i64 {
    let numerator = i128::from(ticks) * i128::from(from.denominator) * i128::from(to.numerator);
    let denominator = i128::from(from.numerator) * i128::from(to.denominator);
    let floor = numerator.div_euclid(denominator);
    clamp(floor + i128::from(numerator.rem_euclid(denominator) != 0))
}

/// Brings a rescaled tick count back into `i64`.
fn clamp(value: i128) -> i64 {
    value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

/// Plans the cuts for `clips`, given the silent spans of each media item.
///
/// `silence` maps a media identifier onto that media's silent spans, in media
/// time; a clip whose media is not in the map is left alone. The result is
/// ordered so that applying it in order is safe: within a track the cuts run
/// last first, so removing one never moves the material a later one names.
///
/// Both rates must be non-zero, which is the Command API's own guarantee for
/// every `rational` it hands out.
pub fn plan(clips: &[Clip], silence: &BTreeMap<String, Vec<Range>>, sequence: Rate) -> Vec<Cut> {
    let mut cuts: Vec<Cut> = Vec::new();
    for clip in clips {
        let Some(spans) = silence.get(&clip.media) else {
            continue;
        };
        if clip.source.rate.numerator == 0 || clip.source.rate.denominator == 0 {
            continue;
        }
        for span in spans {
            if let Some(cut) = cut_for(clip, *span, sequence) {
                cuts.push(cut);
            }
        }
    }

    // Ascending first, so overlapping cuts of one clip — two silences that
    // round onto the same ticks, say — merge into one.
    cuts.sort_by(|left, right| {
        (&left.track, left.start, &left.clip).cmp(&(&right.track, right.start, &right.clip))
    });
    let mut merged: Vec<Cut> = Vec::new();
    for cut in cuts {
        match merged.last_mut() {
            Some(last)
                if last.track == cut.track && last.clip == cut.clip && cut.start <= last.end =>
            {
                last.end = last.end.max(cut.end);
            }
            _ => merged.push(cut),
        }
    }
    // Then descending within each track, which is the order they are applied
    // in: a ripple delete pulls everything after it back, so the cuts after
    // this one must already have happened.
    merged.sort_by(|left, right| (&left.track, right.start).cmp(&(&right.track, left.start)));
    merged
}

/// The sequence-time cut one silent span makes in one clip, if any.
fn cut_for(clip: &Clip, span: Range, sequence: Rate) -> Option<Cut> {
    let start = span.start.max(clip.source.start);
    let end = span.end.min(clip.source.end);
    if end <= start {
        return None;
    }
    let into_clip_start = start - clip.source.start;
    let into_clip_end = end - clip.source.start;
    let cut_start =
        clip.timeline
            .start
            .saturating_add(rescale_ceil(into_clip_start, span.rate, sequence));
    let cut_end =
        clip.timeline
            .start
            .saturating_add(rescale_floor(into_clip_end, span.rate, sequence));
    let cut_start = cut_start.max(clip.timeline.start);
    let cut_end = cut_end.min(clip.timeline.end);
    (cut_end > cut_start).then(|| Cut {
        track: clip.track.clone(),
        clip: clip.id.clone(),
        start: cut_start,
        end: cut_end,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{Clip, Cut, Range, Rate, plan, rescale_ceil, rescale_floor};

    /// 25 fps, the sequence rate every test below uses.
    const SEQUENCE: Rate = Rate {
        numerator: 25,
        denominator: 1,
    };

    /// A clip playing `source` of `media` at `timeline`, on `track`.
    fn clip(id: &str, track: &str, media: &str, timeline: (i64, i64), source: (i64, i64)) -> Clip {
        Clip {
            id: id.to_owned(),
            track: track.to_owned(),
            media: media.to_owned(),
            timeline: Range::new(timeline.0, timeline.1, SEQUENCE),
            source: Range::new(source.0, source.1, Rate::per_second(48_000)),
        }
    }

    /// One media item's silent spans, in seconds at 48 kHz.
    fn silence(media: &str, spans: &[(i64, i64)]) -> BTreeMap<String, Vec<Range>> {
        let mut map = BTreeMap::new();
        map.insert(
            media.to_owned(),
            spans
                .iter()
                .map(|&(start, end)| Range {
                    start,
                    end,
                    rate: Rate::per_second(48_000),
                })
                .collect(),
        );
        map
    }

    #[test]
    fn a_silence_inside_a_clip_becomes_a_cut_in_sequence_time() {
        // The clip plays 10 s of media from its 2 s mark, at the head of the
        // sequence. Silence from 4 s to 6 s of the media is 2 s to 4 s of the
        // clip, which at 25 fps is frames 50 to 100.
        let clips = [clip("c", "t", "m", (0, 250), (96_000, 480_000))];
        let cuts = plan(&clips, &silence("m", &[(192_000, 288_000)]), SEQUENCE);
        assert_eq!(
            cuts,
            vec![Cut {
                track: "t".to_owned(),
                clip: "c".to_owned(),
                start: 50,
                end: 100,
            }]
        );
        assert_eq!(cuts[0].len(), 50);
    }

    #[test]
    fn a_silence_outside_the_clips_source_window_cuts_nothing() {
        let clips = [clip("c", "t", "m", (0, 250), (96_000, 480_000))];
        // Before the in point, and after the out point.
        assert!(plan(&clips, &silence("m", &[(0, 48_000)]), SEQUENCE).is_empty());
        assert!(plan(&clips, &silence("m", &[(600_000, 700_000)]), SEQUENCE).is_empty());
        // Another media item's silence is not this clip's.
        assert!(plan(&clips, &silence("other", &[(192_000, 288_000)]), SEQUENCE).is_empty());
    }

    #[test]
    fn a_silence_overhanging_the_clip_is_clamped_to_what_the_clip_plays() {
        // Clip plays media 2 s .. 12 s at sequence 0 .. 10 s.
        let clips = [clip("c", "t", "m", (0, 250), (96_000, 480_000))];
        // Silence from 1 s to 3 s of the media: only 2 s .. 3 s is in the clip,
        // which is the first 25 frames of it.
        let cuts = plan(&clips, &silence("m", &[(48_000, 144_000)]), SEQUENCE);
        assert_eq!(cuts[0].start, 0);
        assert_eq!(cuts[0].end, 25);
    }

    #[test]
    fn a_cut_never_rounds_outwards_onto_audible_material() {
        // Silence from 2.01 s to 2.99 s of the media, in a clip that starts at
        // its 2 s mark: 0.01 s .. 0.99 s of the clip is frame 0.25 to frame
        // 24.75 at 25 fps, so the cut is frames 1 .. 24.
        let clips = [clip("c", "t", "m", (0, 250), (96_000, 480_000))];
        let cuts = plan(&clips, &silence("m", &[(96_480, 143_520)]), SEQUENCE);
        assert_eq!(cuts[0].start, 1);
        assert_eq!(cuts[0].end, 24);
    }

    #[test]
    fn two_silences_that_land_on_the_same_ticks_merge_into_one_cut() {
        let clips = [clip("c", "t", "m", (0, 250), (0, 480_000))];
        let cuts = plan(
            &clips,
            &silence("m", &[(48_000, 96_000), (96_000, 144_000)]),
            SEQUENCE,
        );
        assert_eq!(cuts.len(), 1);
        assert_eq!((cuts[0].start, cuts[0].end), (25, 75));
    }

    #[test]
    fn cuts_are_ordered_last_first_within_each_track() {
        let clips = [
            clip("a", "t1", "m", (0, 250), (0, 480_000)),
            clip("b", "t1", "m", (250, 250), (0, 480_000)),
            clip("c", "t2", "m", (0, 250), (0, 480_000)),
        ];
        let cuts = plan(&clips, &silence("m", &[(48_000, 96_000)]), SEQUENCE);
        assert_eq!(cuts.len(), 3);
        let track1: Vec<i64> = cuts
            .iter()
            .filter(|cut| cut.track == "t1")
            .map(|cut| cut.start)
            .collect();
        assert_eq!(track1, vec![275, 25]);
    }

    #[test]
    fn a_silence_covering_the_whole_clip_cuts_the_whole_clip() {
        let clips = [clip("c", "t", "m", (100, 250), (0, 480_000))];
        let cuts = plan(&clips, &silence("m", &[(0, 480_000)]), SEQUENCE);
        assert_eq!((cuts[0].start, cuts[0].end), (100, 350));
    }

    #[test]
    fn rescaling_is_exact_and_rounds_the_way_it_says() {
        let from = Rate::per_second(48_000);
        let to = Rate {
            numerator: 24_000,
            denominator: 1_001,
        };
        // One second at 48 kHz is 23.976… frames at 23.976 fps.
        assert_eq!(rescale_floor(48_000, from, to), 23);
        assert_eq!(rescale_ceil(48_000, from, to), 24);
        // An exact conversion rounds to itself either way.
        assert_eq!(rescale_floor(48_000, from, Rate::per_second(25)), 25);
        assert_eq!(rescale_ceil(48_000, from, Rate::per_second(25)), 25);
        // Negative instants round towards negative infinity, not towards zero.
        assert_eq!(rescale_floor(-48_000, from, to), -24);
        assert_eq!(rescale_ceil(-48_000, from, to), -23);
        // A tick count that would overflow saturates rather than wrapping.
        assert_eq!(
            rescale_ceil(i64::MAX, from, Rate::per_second(96_000)),
            i64::MAX
        );
    }
}
