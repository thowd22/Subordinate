//! The inspector: the parameters of the selected clips, as sliders.
//!
//! Every field here maps onto exactly one part of [`SetClipParams`], so the
//! panel has no editing logic of its own: it reads the selected clips, paints
//! their current values, and raises the command the Command API applies. A
//! drag is live — each frame the slider moves raises a command the caller
//! applies straight away, so the viewer recomposites under the pointer — but
//! it is still one entry in the undo stack, because the whole gesture happens
//! inside a [`History::begin_group`]/[`History::commit_group`] pair
//! ([`apply_edit`] is that caller for anyone who wants it done for them).
//!
//! Nothing here holds a float. A slider is a float because egui's is, but the
//! value crosses into the model through [`Fixed6::from_f64`] or, for the
//! fades, as an exact frame count at the sequence timebase; the panel never
//! stores one between frames. The one piece of state it does keep is which
//! field a gesture is open on, which is what tells a release from a change.
//!
//! With several clips selected the fields show the *anchor* — the clip
//! selected first, which is the one the editor was pointing at — and an edit
//! is applied to every selected clip on an unlocked track. Fades are clamped
//! per clip, so dragging a two second fade across a selection holding a one
//! second clip shortens it there rather than failing the whole gesture.

use eframe::egui::{Slider, Ui};
use sub_core::SubResult;
use sub_edit::History;
use sub_edit::commands::SetClipParams;
use sub_model::params::{Fixed6, GainDb, Opacity, Point2, Scale2, Transform};
use sub_model::{Clip, Project, Sequence, Track};
use sub_time::{Rational, RationalTime};

use crate::selection::{ClipRef, Selection};
use crate::timeline_panel::clip_edits_allowed;

/// The furthest a clip may be pushed from the canvas centre, in pixels.
///
/// Wide enough to take a 4K frame entirely off either side of a 4K canvas,
/// which is as far as moving it means anything.
const POSITION_LIMIT: f64 = 8192.0;

/// The smallest scale factor the slider offers.
///
/// Not zero: a zero axis is not a scale the model accepts, because it
/// collapses the clip and makes the transform non-invertible.
const MIN_SCALE: f64 = 0.01;

/// The largest scale factor the slider offers.
const MAX_SCALE: f64 = 10.0;

/// The longest fade the slider offers, in seconds.
///
/// A fade is clamped to the clip anyway; this only bounds the slider when the
/// clip is long.
const MAX_FADE_SECONDS: i64 = 10;

/// One editable parameter in the inspector.
///
/// The label is what the panel paints and what an accessibility query finds
/// it by, so it is also the name a test uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InspectorField {
    /// How opaque the picture is, from 0 to 1.
    Opacity,
    /// Horizontal offset of the clip centre from the canvas centre, in pixels.
    PositionX,
    /// Vertical offset of the clip centre from the canvas centre, in pixels.
    PositionY,
    /// Horizontal scale factor.
    ScaleX,
    /// Vertical scale factor.
    ScaleY,
    /// Clockwise rotation about the clip centre, in degrees.
    Rotation,
    /// Clip audio level, in decibels.
    Gain,
    /// How long the clip ramps up at its head, in frames.
    FadeIn,
    /// How long the clip ramps down at its tail, in frames.
    FadeOut,
}

impl InspectorField {
    /// Every field, in the order the panel paints them.
    pub const ALL: [Self; 9] = [
        Self::Opacity,
        Self::PositionX,
        Self::PositionY,
        Self::ScaleX,
        Self::ScaleY,
        Self::Rotation,
        Self::Gain,
        Self::FadeIn,
        Self::FadeOut,
    ];

    /// The label the slider carries.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Opacity => "Opacity",
            Self::PositionX => "Position X",
            Self::PositionY => "Position Y",
            Self::ScaleX => "Scale X",
            Self::ScaleY => "Scale Y",
            Self::Rotation => "Rotation",
            Self::Gain => "Gain",
            Self::FadeIn => "Fade in",
            Self::FadeOut => "Fade out",
        }
    }

    /// The label the one history entry a gesture on this field produces gets.
    #[must_use]
    pub const fn undo_label(self) -> &'static str {
        match self {
            Self::Opacity => "Change opacity",
            Self::PositionX | Self::PositionY => "Change position",
            Self::ScaleX | Self::ScaleY => "Change scale",
            Self::Rotation => "Change rotation",
            Self::Gain => "Change gain",
            Self::FadeIn => "Change fade in",
            Self::FadeOut => "Change fade out",
        }
    }

    /// The value this field currently holds on `clip`, in slider units.
    ///
    /// `rate` is the sequence timebase, which the two fade fields are counted
    /// in; everything else ignores it.
    #[must_use]
    fn value_of(self, clip: &Clip, rate: Rational) -> f64 {
        match self {
            Self::Opacity => clip.opacity.factor().as_f64(),
            Self::PositionX => clip.transform.position.x.as_f64(),
            Self::PositionY => clip.transform.position.y.as_f64(),
            Self::ScaleX => clip.transform.scale.x().as_f64(),
            Self::ScaleY => clip.transform.scale.y().as_f64(),
            Self::Rotation => clip.transform.rotation_degrees.as_f64(),
            Self::Gain => clip.gain.decibels().as_f64(),
            Self::FadeIn => frames(clip.fade_in, rate),
            Self::FadeOut => frames(clip.fade_out, rate),
        }
    }
}

/// `time` as a whole number of frames at `rate`, for a slider to carry.
#[allow(clippy::cast_precision_loss)]
fn frames(time: RationalTime, rate: Rational) -> f64 {
    time.rescaled_to(rate).value() as f64
}

/// What the inspector asks the Command API to do this frame.
///
/// The three fields are the three moments of a gesture and can all arrive on
/// the same frame: a click that sets a value outright begins, changes and
/// commits at once. [`apply_edit`] is the reference reading of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InspectorResponse {
    /// A gesture began this frame; open a history group with this label.
    pub begin: Option<String>,
    /// The commands to apply now, one per edited clip, in selection order.
    ///
    /// Applying them is what makes the edit live: the project changes under
    /// the pointer and the viewer recomposites from it.
    pub commands: Vec<SetClipParams>,
    /// The gesture ended this frame; commit the open group as one undo step.
    pub commit: bool,
}

impl InspectorResponse {
    /// Whether the frame asked for nothing at all, which is the usual case.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.begin.is_none() && self.commands.is_empty() && !self.commit
    }
}

/// The inspector panel.
///
/// It owns no parameter values: the project is the only copy, so a value
/// changed by an undo, by the MCP bridge or by another panel shows up on the
/// next frame with no syncing. The gesture is the whole of its state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InspectorPanel {
    /// The field a drag or a text edit is currently open on, if any.
    gesture: Option<InspectorField>,
}

impl InspectorPanel {
    /// A panel with no gesture open.
    #[must_use]
    pub const fn new() -> Self {
        Self { gesture: None }
    }

    /// The field a gesture is open on, if one is.
    #[must_use]
    pub const fn gesture(&self) -> Option<InspectorField> {
        self.gesture
    }

    /// Paints the parameters of `selection` and returns what it asks for.
    ///
    /// The panel mutates nothing: the caller applies
    /// [`InspectorResponse::commands`] through the Command API, inside the
    /// group the response's `begin` and `commit` delimit.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        sequence: &Sequence,
        selection: &Selection,
    ) -> InspectorResponse {
        let mut response = InspectorResponse::default();
        let Some(anchor) = anchor_clip(sequence, selection) else {
            ui.label(if selection.is_empty() {
                "Select a clip to edit its parameters."
            } else {
                "The selected clips are no longer in this sequence."
            });
            self.gesture = None;
            return response;
        };

        let rate = sequence.settings.frame_rate;
        let editable: Vec<ClipRef> = selection
            .items()
            .iter()
            .copied()
            .filter(|item| track_of(sequence, *item).is_some_and(clip_edits_allowed))
            .collect();

        ui.heading(&anchor.name);
        if selection.len() > 1 {
            ui.label(format!("{} clips selected", selection.len()));
        }
        if editable.is_empty() {
            ui.label("Every selected clip is on a locked track.");
        }

        ui.add_enabled_ui(!editable.is_empty(), |ui| {
            for field in InspectorField::ALL {
                self.field_ui(ui, field, sequence, anchor, &editable, rate, &mut response);
            }
        });
        response
    }

    /// Paints one field and folds what it raised into `response`.
    #[allow(clippy::too_many_arguments)]
    fn field_ui(
        &mut self,
        ui: &mut Ui,
        field: InspectorField,
        sequence: &Sequence,
        anchor: &Clip,
        editable: &[ClipRef],
        rate: Rational,
        response: &mut InspectorResponse,
    ) {
        let mut value = field.value_of(anchor, rate);
        let widget = match field {
            InspectorField::Opacity => Slider::new(&mut value, 0.0..=1.0).fixed_decimals(3),
            InspectorField::PositionX | InspectorField::PositionY => {
                Slider::new(&mut value, -POSITION_LIMIT..=POSITION_LIMIT).fixed_decimals(1)
            }
            InspectorField::ScaleX | InspectorField::ScaleY => {
                Slider::new(&mut value, MIN_SCALE..=MAX_SCALE).fixed_decimals(3)
            }
            InspectorField::Rotation => Slider::new(&mut value, -360.0..=360.0)
                .fixed_decimals(1)
                .suffix("°"),
            InspectorField::Gain => {
                Slider::new(&mut value, GainDb::MIN.as_f64()..=GainDb::MAX.as_f64())
                    .fixed_decimals(1)
                    .suffix(" dB")
            }
            InspectorField::FadeIn | InspectorField::FadeOut => {
                Slider::new(&mut value, 0.0..=fade_limit(rate)).fixed_decimals(0)
            }
        };
        let painted = ui.add(widget.text(field.label()));

        if painted.changed() {
            if self.gesture.is_none() {
                self.gesture = Some(field);
                response.begin = Some(field.undo_label().to_owned());
            }
            response
                .commands
                .extend(commands_for(field, value, sequence, editable, rate));
        }
        // The gesture ends when the pointer lets go, when the widget loses
        // focus, or straight away when the change came from something that is
        // not a drag at all: a click on the slider track, an arrow key, a
        // typed value. A drag in progress is the only thing that holds it
        // open, so a gesture can never be left dangling.
        let still_dragging = painted.dragged() || painted.drag_started();
        let released = painted.drag_stopped()
            || painted.lost_focus()
            || (painted.changed() && !still_dragging);
        if self.gesture == Some(field) && released {
            self.gesture = None;
            response.commit = true;
        }
    }
}

/// The longest fade the slider offers at `rate`, in frames.
#[allow(clippy::cast_precision_loss)]
fn fade_limit(rate: Rational) -> f64 {
    RationalTime::from_seconds(MAX_FADE_SECONDS)
        .rescaled_to(rate)
        .value() as f64
}

/// The clip the fields show: the first selected one still in `sequence`.
fn anchor_clip<'a>(sequence: &'a Sequence, selection: &Selection) -> Option<&'a Clip> {
    selection
        .items()
        .iter()
        .find_map(|item| track_of(sequence, *item)?.clip(item.clip))
}

/// The track `item` names, if `sequence` still holds it.
fn track_of(sequence: &Sequence, item: ClipRef) -> Option<&Track> {
    sequence
        .tracks
        .iter()
        .find(|track| track.id == item.track && track.clip(item.clip).is_some())
}

/// One [`SetClipParams`] per clip in `editable`, setting `field` to `value`.
///
/// A value egui cannot hand to the model — a scale of zero, a gain past the
/// fader — yields no command for that clip rather than a command that would
/// be refused; the sliders are bounded so this does not happen in practice.
fn commands_for(
    field: InspectorField,
    value: f64,
    sequence: &Sequence,
    editable: &[ClipRef],
    rate: Rational,
) -> Vec<SetClipParams> {
    editable
        .iter()
        .filter_map(|item| {
            let clip = track_of(sequence, *item)?.clip(item.clip)?;
            let command = SetClipParams::new(sequence.id, item.track, item.clip);
            Some(match field {
                InspectorField::Opacity => command.with_opacity(Opacity::from_f64(value).ok()?),
                InspectorField::PositionX => command.with_transform(Transform {
                    position: Point2::new(Fixed6::from_f64(value).ok()?, clip.transform.position.y),
                    ..clip.transform
                }),
                InspectorField::PositionY => command.with_transform(Transform {
                    position: Point2::new(clip.transform.position.x, Fixed6::from_f64(value).ok()?),
                    ..clip.transform
                }),
                InspectorField::ScaleX => command.with_transform(Transform {
                    scale: Scale2::new(Fixed6::from_f64(value).ok()?, clip.transform.scale.y())
                        .ok()?,
                    ..clip.transform
                }),
                InspectorField::ScaleY => command.with_transform(Transform {
                    scale: Scale2::new(clip.transform.scale.x(), Fixed6::from_f64(value).ok()?)
                        .ok()?,
                    ..clip.transform
                }),
                InspectorField::Rotation => command.with_transform(Transform {
                    rotation_degrees: Fixed6::from_f64(value).ok()?,
                    ..clip.transform
                }),
                InspectorField::Gain => command.with_gain(GainDb::from_f64(value).ok()?),
                InspectorField::FadeIn => {
                    command.with_fade_in(clamped_fade(value, clip, clip.fade_out, rate))
                }
                InspectorField::FadeOut => {
                    command.with_fade_out(clamped_fade(value, clip, clip.fade_in, rate))
                }
            })
        })
        .collect()
}

/// `value` frames of fade, clamped to what is left of `clip` beside `other`.
///
/// The model refuses fades that together outlast the clip, and a selection
/// holds clips of different lengths, so the clamp is per clip rather than a
/// bound on the slider.
#[allow(clippy::cast_possible_truncation)]
fn clamped_fade(value: f64, clip: &Clip, other: RationalTime, rate: Rational) -> RationalTime {
    let wanted = RationalTime::new(value.round() as i64, rate);
    let room = clip
        .duration()
        .rescaled_to(rate)
        .checked_sub(other.rescaled_to(rate))
        .unwrap_or_else(|| RationalTime::zero(rate));
    if room.is_negative() {
        return RationalTime::zero(rate);
    }
    if wanted.is_negative() {
        RationalTime::zero(rate)
    } else if wanted > room {
        room
    } else {
        wanted
    }
}

/// Applies what the inspector raised, keeping a whole gesture to one undo
/// step.
///
/// This is the reference caller: it opens the group `begin` names, applies
/// every command in order, and commits on release. A gesture that changed
/// nothing commits nothing, so a slider clicked and released where it already
/// was leaves the undo stack alone.
///
/// # Errors
///
/// Returns whatever the Command API returns. A command that fails aborts the
/// open group, which puts the project back as it was before the gesture, so
/// the caller never sees a half-applied drag.
pub fn apply_edit(
    history: &mut History,
    project: &mut Project,
    response: &InspectorResponse,
) -> SubResult<()> {
    if let Some(label) = &response.begin {
        history.begin_group(label.clone())?;
    }
    for command in &response.commands {
        history.apply(project, *command)?;
    }
    if response.commit {
        history.commit_group()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_model::sequence::SequenceSettings;
    use sub_model::{Clip, MediaItem, MediaPath, Track, TrackItem, TrackKind};
    use sub_time::TimeRange;

    /// A sequence with one video track holding `count` clips of 48 frames.
    fn scene(count: usize) -> (Project, Sequence) {
        let mut project = Project::new("inspector");
        let item = MediaItem::new(MediaPath::new("media/take.mp4").expect("valid path"));
        let media = item.id;
        project.media.push(item);

        let rate = Rational::FPS_24;
        let mut track = Track::new("V1", TrackKind::Video);
        for index in 0..count {
            let source = TimeRange::new(RationalTime::new(0, rate), RationalTime::new(48, rate))
                .expect("valid source range");
            track.items.push(TrackItem::Clip(Clip::new(
                format!("take {index}"),
                media,
                source,
            )));
        }
        let mut sequence = Sequence::new("edit", SequenceSettings::default());
        sequence.tracks.push(track);
        project.sequences.push(sequence.clone());
        (project, sequence)
    }

    /// The whole selection of `sequence`'s first track.
    fn select_all(sequence: &Sequence) -> Selection {
        let track = &sequence.tracks[0];
        let mut selection = Selection::new();
        for clip in track.clips() {
            selection.toggle(ClipRef::new(track.id, clip.id));
        }
        selection
    }

    #[test]
    fn a_field_builds_one_command_per_selected_clip() {
        let (_, sequence) = scene(3);
        let selection = select_all(&sequence);
        let editable: Vec<_> = selection.items().to_vec();
        let commands = commands_for(
            InspectorField::Opacity,
            0.25,
            &sequence,
            &editable,
            Rational::FPS_24,
        );
        assert_eq!(commands.len(), 3, "every selected clip is edited");
        for command in &commands {
            assert_eq!(
                command.opacity.expect("the opacity is named").factor(),
                Fixed6::from_micros(250_000)
            );
            assert!(command.transform.is_none(), "nothing else is touched");
        }
    }

    #[test]
    fn a_transform_field_keeps_the_axes_it_does_not_edit() {
        let (_, sequence) = scene(1);
        let selection = select_all(&sequence);
        let editable: Vec<_> = selection.items().to_vec();
        let commands = commands_for(
            InspectorField::PositionX,
            120.0,
            &sequence,
            &editable,
            Rational::FPS_24,
        );
        let transform = commands[0].transform.expect("the transform is named");
        assert_eq!(transform.position.x, Fixed6::from_units(120));
        assert_eq!(transform.position.y, Fixed6::ZERO);
        assert_eq!(transform.scale, Scale2::UNIFORM, "the scale is untouched");
    }

    #[test]
    fn a_fade_is_clamped_to_what_is_left_of_the_clip() {
        let (_, sequence) = scene(1);
        let selection = select_all(&sequence);
        let editable: Vec<_> = selection.items().to_vec();
        let rate = Rational::FPS_24;
        // The clip is 48 frames long, so a 240 frame fade cannot fit.
        let commands = commands_for(InspectorField::FadeIn, 240.0, &sequence, &editable, rate);
        assert_eq!(
            commands[0].fade_in.expect("a fade"),
            RationalTime::new(48, rate),
            "the fade is cut to the clip"
        );
    }

    #[test]
    fn a_locked_track_is_left_out_of_the_edit() {
        let (_, mut sequence) = scene(1);
        let selection = select_all(&sequence);
        sequence.tracks[0].locked = true;
        let editable: Vec<ClipRef> = selection
            .items()
            .iter()
            .copied()
            .filter(|item| track_of(&sequence, *item).is_some_and(clip_edits_allowed))
            .collect();
        assert!(editable.is_empty(), "a locked track offers nothing to edit");
    }

    #[test]
    fn the_anchor_is_the_clip_selected_first() {
        let (_, sequence) = scene(2);
        let track = &sequence.tracks[0];
        let second = track.clips().nth(1).expect("a second clip").id;
        let first = track.clips().next().expect("a first clip").id;
        let mut selection = Selection::new();
        selection.toggle(ClipRef::new(track.id, second));
        selection.toggle(ClipRef::new(track.id, first));
        assert_eq!(
            anchor_clip(&sequence, &selection).expect("an anchor").id,
            second
        );
    }

    #[test]
    fn an_empty_response_asks_for_nothing() {
        assert!(InspectorResponse::default().is_empty());
    }
}
