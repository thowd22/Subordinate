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
//!
//! # Effects
//!
//! Under the sliders is the anchor clip's effect stack: what runs on it, in
//! the order the compositor runs it. The rows are built from two sources, as
//! [`crate::effects`] describes — the project says which plugins are applied
//! and what is bound, the catalogue says what each plugin declared — so a
//! control is generated from the plugin's own `param-desc` and nothing about
//! an effect is stored twice.
//!
//! Effect edits are the one thing here that is not applied across the
//! selection: an effect is identified by the instance on one clip, so adding,
//! reordering, removing and binding all act on the anchor. They are raised the
//! same way the sliders are, as commands in [`InspectorResponse::effects`] the
//! caller applies, and grouped the same way: a click is one undo step, a drag
//! on a parameter is one undo step for the whole gesture.

use eframe::egui::{Button, ComboBox, Response, Slider, Ui};
use sub_core::SubResult;
use sub_edit::History;
use sub_edit::commands::{
    AddClipEffect, MoveClipEffect, RemoveClipEffect, SetClipEffectParam, SetClipParams,
};
use sub_model::effect::{ClipEffect, EffectValue};
use sub_model::params::{Fixed6, GainDb, Opacity, Point2, Scale2, Transform};
use sub_model::{Clip, ClipId, EffectId, Project, Sequence, SequenceId, Track, TrackId};
use sub_render::{EffectParam, ParamKind};
use sub_time::{Rational, RationalTime};

use crate::effects::EffectCatalog;
use crate::selection::{ClipRef, Selection};
use crate::timeline_panel::clip_edits_allowed;

/// The heading the effect stack sits under.
pub const EFFECTS_HEADING: &str = "Effects";

/// What the effect list shows when the clip carries none.
pub const NO_EFFECTS_LABEL: &str = "No effects on this clip.";

/// The heading the add-effect picker sits under.
pub const ADD_EFFECT_LABEL: &str = "Add effect";

/// What the picker shows when no effect plugin is installed.
pub const NO_EFFECT_PLUGINS_LABEL: &str = "No effect plugins installed.";

/// The button that moves an effect one place earlier in the stack.
pub const MOVE_UP_LABEL: &str = "Move up";

/// The button that moves an effect one place later in the stack.
pub const MOVE_DOWN_LABEL: &str = "Move down";

/// The button that takes an effect off the clip.
pub const REMOVE_EFFECT_LABEL: &str = "Remove";

/// What an effect with no declaration to read shows instead of controls.
pub const UNDECLARED_LABEL: &str = "Parameters appear once the plugin loads.";

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
    /// The effect-stack commands to apply now, in the order they were raised.
    ///
    /// They always name the anchor clip, because an effect is an instance on
    /// one clip rather than a value every selected clip can share.
    pub effects: Vec<EffectEdit>,
    /// The gesture ended this frame; commit the open group as one undo step.
    pub commit: bool,
}

impl InspectorResponse {
    /// Whether the frame asked for nothing at all, which is the usual case.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.begin.is_none() && self.commands.is_empty() && self.effects.is_empty() && !self.commit
    }
}

/// One edit to the anchor clip's effect stack, as the command that performs it.
///
/// The panel raises these rather than applying them, exactly as it does with
/// [`SetClipParams`], so the same edit can be applied through the engine, a
/// bare [`History`] or nothing at all in a test that only asserts on what was
/// asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectEdit {
    /// Apply a plugin's effect to the clip, at the end of the stack.
    Add(AddClipEffect),
    /// Move an effect to another place in the stack.
    Move(MoveClipEffect),
    /// Take an effect off the clip.
    Remove(RemoveClipEffect),
    /// Bind one parameter of one applied effect.
    SetParam(SetClipEffectParam),
}

impl EffectEdit {
    /// The label the one history entry this edit produces gets.
    #[must_use]
    pub const fn undo_label(&self) -> &'static str {
        match self {
            Self::Add(_) => "Add effect",
            Self::Move(_) => "Reorder effect",
            Self::Remove(_) => "Remove effect",
            Self::SetParam(_) => "Change effect parameter",
        }
    }

    /// Applies the edit through `history`, as one command.
    ///
    /// # Errors
    ///
    /// Whatever the command returns: an effect the clip no longer holds, a
    /// locked track, a plugin id the model refuses.
    pub fn apply(&self, history: &mut History, project: &mut Project) -> SubResult<()> {
        match self {
            Self::Add(command) => history.apply(project, command.clone()),
            Self::Move(command) => history.apply(project, *command),
            Self::Remove(command) => history.apply(project, *command),
            Self::SetParam(command) => history.apply(project, command.clone()),
        }
    }
}

/// The widget a parameter gesture is open on.
///
/// A colour is four sliders, so the channel is part of the identity: letting
/// go of the red slider must not commit a gesture opened on the green one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParamKey {
    /// The applied effect the parameter belongs to.
    effect: EffectId,
    /// The parameter id, as the plugin declared it.
    param: String,
    /// Which channel of a colour, when the parameter is one.
    channel: Option<usize>,
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
    /// The effect parameter a drag is currently open on, if any.
    param_gesture: Option<ParamKey>,
}

impl InspectorPanel {
    /// A panel with no gesture open.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            gesture: None,
            param_gesture: None,
        }
    }

    /// The field a gesture is open on, if one is.
    #[must_use]
    pub const fn gesture(&self) -> Option<InspectorField> {
        self.gesture
    }

    /// Whether a gesture is open on an effect parameter.
    #[must_use]
    pub const fn editing_effect_param(&self) -> bool {
        self.param_gesture.is_some()
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
        catalog: &EffectCatalog,
    ) -> InspectorResponse {
        let mut response = InspectorResponse::default();
        let anchor_ref = anchor_ref(sequence, selection);
        let Some(anchor) = anchor_ref.and_then(|item| track_of(sequence, item)?.clip(item.clip))
        else {
            ui.label(if selection.is_empty() {
                "Select a clip to edit its parameters."
            } else {
                "The selected clips are no longer in this sequence."
            });
            self.gesture = None;
            self.param_gesture = None;
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
            ui.separator();
            ui.heading(EFFECTS_HEADING);
            // The anchor is the clip whose stack is shown, so its own track is
            // what decides whether the stack can be edited.
            let target = anchor_ref.filter(|item| editable.contains(item));
            if let Some(target) = target {
                self.effects_ui(ui, sequence.id, target, anchor, catalog, &mut response);
            }
        });
        response
    }

    /// Paints the anchor clip's effect stack and the add-effect picker.
    fn effects_ui(
        &mut self,
        ui: &mut Ui,
        sequence: SequenceId,
        target: ClipRef,
        anchor: &Clip,
        catalog: &EffectCatalog,
        response: &mut InspectorResponse,
    ) {
        if anchor.effects.is_empty() {
            ui.label(NO_EFFECTS_LABEL);
        }
        let last = anchor.effects.len().saturating_sub(1);
        for (index, effect) in anchor.effects.iter().enumerate() {
            ui.push_id(effect.id, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{}. {}",
                        index + 1,
                        catalog.name_of(&effect.plugin)
                    ));
                    if ui
                        .add_enabled(index > 0, Button::new(MOVE_UP_LABEL))
                        .clicked()
                    {
                        raise(
                            response,
                            EffectEdit::Move(MoveClipEffect::new(
                                sequence,
                                target.track,
                                target.clip,
                                effect.id,
                                index - 1,
                            )),
                        );
                    }
                    if ui
                        .add_enabled(index < last, Button::new(MOVE_DOWN_LABEL))
                        .clicked()
                    {
                        raise(
                            response,
                            EffectEdit::Move(MoveClipEffect::new(
                                sequence,
                                target.track,
                                target.clip,
                                effect.id,
                                index + 1,
                            )),
                        );
                    }
                    if ui.button(REMOVE_EFFECT_LABEL).clicked() {
                        raise(
                            response,
                            EffectEdit::Remove(RemoveClipEffect::new(
                                sequence,
                                target.track,
                                target.clip,
                                effect.id,
                            )),
                        );
                    }
                });
                let params = catalog.params_of(&effect.plugin);
                if params.is_empty() {
                    ui.label(UNDECLARED_LABEL);
                }
                for param in params {
                    self.param_ui(ui, sequence, target, effect, param, response);
                }
            });
        }

        ui.separator();
        ui.label(ADD_EFFECT_LABEL);
        if catalog.is_empty() {
            ui.label(NO_EFFECT_PLUGINS_LABEL);
        }
        for listing in catalog.entries() {
            if ui.button(&listing.name).clicked() {
                raise(
                    response,
                    EffectEdit::Add(AddClipEffect::new(
                        sequence,
                        target.track,
                        target.clip,
                        listing.plugin.clone(),
                    )),
                );
            }
        }
    }

    /// Paints one declared parameter of one applied effect.
    ///
    /// The widget comes from the declaration: a range is a slider, a flag is a
    /// checkbox, a closed set is a combo box and a colour is four channel
    /// sliders. The value comes from the project when the effect binds one and
    /// from the declaration's default when it does not, which is exactly what
    /// the compositor binds.
    fn param_ui(
        &mut self,
        ui: &mut Ui,
        sequence: SequenceId,
        target: ClipRef,
        effect: &ClipEffect,
        param: &EffectParam,
        response: &mut InspectorResponse,
    ) {
        let bind = |value: EffectValue| {
            SetClipEffectParam::new(
                sequence,
                target.track,
                target.clip,
                effect.id,
                param.id.clone(),
                value,
            )
        };
        let key = |channel| ParamKey {
            effect: effect.id,
            param: param.id.clone(),
            channel,
        };

        match param.kind {
            ParamKind::Float {
                min,
                max,
                default,
                step,
            } => {
                let mut value = float_of(effect, &param.id, default);
                let mut slider =
                    Slider::new(&mut value, f64::from(min)..=f64::from(max)).text(&param.label);
                if let Some(step) = step {
                    slider = slider.step_by(f64::from(step));
                }
                let painted = ui.add(slider);
                let command = Fixed6::from_f64(value)
                    .ok()
                    .map(|fixed| bind(EffectValue::Float(fixed)));
                self.param_gesture_ui(&painted, &key(None), command, response);
            }
            ParamKind::Int { min, max, default } => {
                let mut value = int_of(effect, &param.id, default);
                let painted = ui.add(Slider::new(&mut value, min..=max).text(&param.label));
                self.param_gesture_ui(
                    &painted,
                    &key(None),
                    Some(bind(EffectValue::Int(value))),
                    response,
                );
            }
            ParamKind::Bool { default } => {
                let mut value = bool_of(effect, &param.id, default);
                let painted = ui.checkbox(&mut value, &param.label);
                self.param_gesture_ui(
                    &painted,
                    &key(None),
                    Some(bind(EffectValue::Bool(value))),
                    response,
                );
            }
            ParamKind::Choice {
                ref variants,
                default,
            } => {
                let mut chosen = choice_of(effect, &param.id, default, variants.len());
                let selected = variants
                    .get(usize::try_from(chosen).unwrap_or(0))
                    .map_or("", String::as_str);
                let painted = ComboBox::from_label(&param.label)
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        let mut changed = false;
                        for (index, variant) in variants.iter().enumerate() {
                            let index = u32::try_from(index).unwrap_or(u32::MAX);
                            changed |= ui.selectable_value(&mut chosen, index, variant).changed();
                        }
                        changed
                    });
                if painted.inner == Some(true) {
                    raise(
                        response,
                        EffectEdit::SetParam(bind(EffectValue::Choice(chosen))),
                    );
                }
            }
            ParamKind::Color { default } => {
                let mut channels = color_of(effect, &param.id, default);
                for (channel, name) in ["R", "G", "B", "A"].into_iter().enumerate() {
                    let mut value = channels[channel].as_f64();
                    let painted = ui.add(
                        Slider::new(&mut value, 0.0..=1.0)
                            .fixed_decimals(3)
                            .text(format!("{} {name}", param.label)),
                    );
                    let command = Fixed6::from_f64(value).ok().map(|fixed| {
                        channels[channel] = fixed;
                        bind(EffectValue::Color(channels))
                    });
                    self.param_gesture_ui(&painted, &key(Some(channel)), command, response);
                }
            }
        }
    }

    /// Folds one parameter widget's response into the frame's gesture.
    ///
    /// The same three moments the sliders above have: the first change opens a
    /// group, every change raises a command so the edit is live, and letting
    /// go commits the group as one undo step.
    fn param_gesture_ui(
        &mut self,
        painted: &Response,
        key: &ParamKey,
        command: Option<SetClipEffectParam>,
        response: &mut InspectorResponse,
    ) {
        if painted.changed() {
            if self.param_gesture.is_none() && self.gesture.is_none() {
                self.param_gesture = Some(key.clone());
                response.begin = Some("Change effect parameter".to_owned());
            }
            if let Some(command) = command {
                response.effects.push(EffectEdit::SetParam(command));
            }
        }
        let still_dragging = painted.dragged() || painted.drag_started();
        let released = painted.drag_stopped()
            || painted.lost_focus()
            || (painted.changed() && !still_dragging);
        if self.param_gesture.as_ref() == Some(key) && released {
            self.param_gesture = None;
            response.commit = true;
        }
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

/// Raises one whole effect edit: its own history group, opened and committed
/// on the frame the button was clicked.
///
/// A click cannot arrive while a drag is open — the pointer is doing one thing
/// at a time — so there is never a group to interleave with.
fn raise(response: &mut InspectorResponse, edit: EffectEdit) {
    if response.begin.is_none() {
        response.begin = Some(edit.undo_label().to_owned());
    }
    response.effects.push(edit);
    response.commit = true;
}

/// The clip the fields show and whose effect stack is listed: the first
/// selected clip still in `sequence`.
fn anchor_ref(sequence: &Sequence, selection: &Selection) -> Option<ClipRef> {
    selection
        .items()
        .iter()
        .copied()
        .find(|item| track_of(sequence, *item).is_some())
}

/// The value bound to `param`, or the plugin's declared default.
fn float_of(effect: &ClipEffect, param: &str, default: f32) -> f64 {
    match effect.param(param) {
        Some(EffectValue::Float(value)) => value.as_f64(),
        _ => f64::from(default),
    }
}

/// The whole number bound to `param`, or the plugin's declared default.
fn int_of(effect: &ClipEffect, param: &str, default: i32) -> i32 {
    match effect.param(param) {
        Some(EffectValue::Int(value)) => value,
        _ => default,
    }
}

/// The flag bound to `param`, or the plugin's declared default.
fn bool_of(effect: &ClipEffect, param: &str, default: bool) -> bool {
    match effect.param(param) {
        Some(EffectValue::Bool(value)) => value,
        _ => default,
    }
}

/// The chosen variant of `param`, or the declared default.
///
/// A bound index the declaration no longer holds — a plugin that dropped a
/// variant — falls back to the default rather than painting an empty combo.
fn choice_of(effect: &ClipEffect, param: &str, default: u32, variants: usize) -> u32 {
    let in_range = |index: u32| usize::try_from(index).is_ok_and(|index| index < variants);
    match effect.param(param) {
        Some(EffectValue::Choice(index)) if in_range(index) => index,
        _ => default,
    }
}

/// The colour bound to `param`, or the plugin's declared default.
///
/// A declared channel outside `0..=1` cannot be held exactly, so the fallback
/// is the nearest value [`Fixed6`] can carry rather than a refusal: the
/// declaration is a plugin's input and the panel always has something to
/// paint.
fn color_of(effect: &ClipEffect, param: &str, default: [f32; 4]) -> [Fixed6; 4] {
    match effect.param(param) {
        Some(EffectValue::Color(channels)) => channels,
        _ => default.map(|channel| {
            Fixed6::from_f64(f64::from(channel.clamp(0.0, 1.0))).unwrap_or(Fixed6::ZERO)
        }),
    }
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
    for edit in &response.effects {
        edit.apply(history, project)?;
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
            anchor_ref(&sequence, &selection).expect("an anchor").clip,
            second
        );
    }

    #[test]
    fn an_empty_response_asks_for_nothing() {
        assert!(InspectorResponse::default().is_empty());
    }

    #[test]
    fn a_response_carrying_only_an_effect_edit_is_not_empty() {
        let response = InspectorResponse {
            effects: vec![EffectEdit::Add(AddClipEffect::new(
                SequenceId::new(),
                TrackId::new(),
                ClipId::new(),
                "com.example.grade",
            ))],
            ..InspectorResponse::default()
        };
        assert!(!response.is_empty());
    }

    #[test]
    fn a_parameter_takes_the_plugins_default_until_the_effect_binds_one() {
        let effect = ClipEffect::new("com.example.grade").expect("a valid plugin id");
        assert!((float_of(&effect, "exposure", 0.25) - 0.25).abs() < f64::EPSILON);
        assert_eq!(int_of(&effect, "radius", 4), 4);
        assert!(bool_of(&effect, "invert", true));

        let bound = effect
            .clone()
            .with_param("exposure", EffectValue::Float(Fixed6::ONE))
            .with_param("radius", EffectValue::Int(9))
            .with_param("invert", EffectValue::Bool(false));
        assert!((float_of(&bound, "exposure", 0.25) - 1.0).abs() < f64::EPSILON);
        assert_eq!(int_of(&bound, "radius", 4), 9);
        assert!(!bool_of(&bound, "invert", true));
    }

    #[test]
    fn a_bound_value_of_the_wrong_shape_falls_back_to_the_declaration() {
        // A plugin that changed a parameter from a choice to a float leaves an
        // old project binding a choice; the panel paints the new declaration.
        let effect = ClipEffect::new("com.example.grade")
            .expect("a valid plugin id")
            .with_param("mode", EffectValue::Choice(7));
        assert_eq!(
            choice_of(&effect, "mode", 1, 3),
            1,
            "an index the declaration no longer holds falls back"
        );
        assert!((float_of(&effect, "mode", 0.5) - 0.5).abs() < f64::EPSILON);
        assert_eq!(choice_of(&effect, "mode", 1, 8), 7, "one it holds is kept");
    }

    #[test]
    fn a_declared_colour_crosses_into_exact_channels() {
        let effect = ClipEffect::new("com.example.grade").expect("a valid plugin id");
        assert_eq!(
            color_of(&effect, "tint", [1.0, 0.5, 0.0, 2.0]),
            [
                Fixed6::ONE,
                Fixed6::from_micros(500_000),
                Fixed6::ZERO,
                Fixed6::ONE
            ],
            "a channel outside the range is clamped rather than refused"
        );
    }

    #[test]
    fn every_effect_edit_names_the_history_entry_it_makes() {
        let sequence = SequenceId::new();
        let track = TrackId::new();
        let clip = ClipId::new();
        let effect = EffectId::new();
        assert_eq!(
            EffectEdit::Add(AddClipEffect::new(
                sequence,
                track,
                clip,
                "com.example.grade"
            ))
            .undo_label(),
            "Add effect"
        );
        assert_eq!(
            EffectEdit::Move(MoveClipEffect::new(sequence, track, clip, effect, 0)).undo_label(),
            "Reorder effect"
        );
        assert_eq!(
            EffectEdit::Remove(RemoveClipEffect::new(sequence, track, clip, effect)).undo_label(),
            "Remove effect"
        );
        assert_eq!(
            EffectEdit::SetParam(SetClipEffectParam::new(
                sequence,
                track,
                clip,
                effect,
                "exposure",
                EffectValue::Int(1),
            ))
            .undo_label(),
            "Change effect parameter"
        );
    }
}
