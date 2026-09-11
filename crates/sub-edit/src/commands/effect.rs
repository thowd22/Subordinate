//! The clip effect stack commands: add, insert, remove, reorder and bind.
//!
//! A clip's [`effects`](sub_model::Clip::effects) is an ordered list of plugin
//! references (see [`sub_model::effect`]): what runs on the clip, in the order
//! the compositor runs it, with the parameter values the user moved away from
//! the plugin's declared defaults. Every edit the inspector offers on that
//! list is one of the commands here, so an agent editing an effect stack over
//! the Command API and a user dragging a slider in the inspector go through
//! exactly the same mutations.
//!
//! The two conventions of [`super`] hold here as they do everywhere else.
//! Removing an effect returns [`InsertClipEffect`] carrying the whole
//! [`ClipEffect`] and the index it sat at, so undo restores its identifier,
//! its order and every bound value rather than something that merely looks the
//! same; and every lookup fails with a stable code — `edit.effect_not_found`,
//! `edit.duplicate_effect`, `edit.invalid_index` — before anything is written.
//!
//! Nothing here knows what a plugin declares. A parameter id the installed
//! plugin does not declare is kept in the project and ignored when binding
//! (`sub-plugin` resolves that), so an effect stack outlives an edit to a
//! plugin's parameter list; what the commands do enforce is the model's own
//! spelling rules, through [`ClipEffect::validate`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_model::effect::{ClipEffect, EffectValue};
use sub_model::{Clip, ClipId, EffectId, Project, SequenceId, TrackId};

use super::{check_insert_index, clip_mut};
use crate::codes;
use crate::{Command, Inverse};

/// Applies a plugin effect to a clip, at the end of its stack.
///
/// The effect is created here rather than by the caller, so the identifier is
/// the command's own: undoing gives back [`RemoveClipEffect`], whose inverse
/// carries the whole effect, which is what makes a redo restore the same
/// identity and the same bound values.
///
/// ```
/// use sub_edit::History;
/// use sub_edit::commands::AddClipEffect;
/// use sub_model::{
///     Clip, MediaItem, MediaPath, Project, Sequence, SequenceSettings, Track, TrackKind,
/// };
/// use sub_time::{Rational, RationalTime, TimeRange};
///
/// let mut project = Project::new("Doc cut");
/// let media = MediaItem::new(MediaPath::new("a.mp4").unwrap());
/// let media_id = media.id;
/// project.media.push(media);
///
/// let rate = Rational::FPS_24;
/// let source = TimeRange::new(RationalTime::zero(rate), RationalTime::new(48, rate)).unwrap();
/// let clip = Clip::new("shot 1", media_id, source);
/// let clip_id = clip.id;
/// let mut track = Track::new("V1", TrackKind::Video);
/// track.items.push(clip.into());
/// let track_id = track.id;
/// let mut sequence = Sequence::new("Main", SequenceSettings::default());
/// sequence.tracks.push(track);
/// let sequence_id = sequence.id;
/// project.sequences.push(sequence);
///
/// let mut history = History::new();
/// history
///     .apply(
///         &mut project,
///         AddClipEffect::new(sequence_id, track_id, clip_id, "com.example.grade"),
///     )
///     .unwrap();
/// let effects = |project: &Project| {
///     project.sequences[0].tracks[0].clip(clip_id).unwrap().effects.clone()
/// };
/// assert_eq!(effects(&project)[0].plugin, "com.example.grade");
///
/// history.undo(&mut project).unwrap();
/// assert!(effects(&project).is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddClipEffect {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip the effect is applied to.
    pub clip: ClipId,
    /// Reverse-DNS id of the plugin declaring the effect.
    pub plugin: String,
}

impl AddClipEffect {
    /// Applies `plugin`'s effect to `clip`.
    #[must_use]
    pub fn new(
        sequence: SequenceId,
        track: TrackId,
        clip: ClipId,
        plugin: impl Into<String>,
    ) -> Self {
        Self {
            sequence,
            track,
            clip,
            plugin: plugin.into(),
        }
    }
}

impl Command for AddClipEffect {
    const KIND: &'static str = "clip.add_effect";
    const DESCRIPTION: &'static str =
        "Apply a plugin effect to a clip, at the end of its effect stack.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let effect = ClipEffect::new(self.plugin.clone())?;
        let id = effect.id;
        let clip = clip_mut(project, self.sequence, self.track, self.clip)?;
        let mut candidate = clip.clone();
        candidate.effects.push(effect);
        candidate.validate()?;
        *clip = candidate;
        Ok(Inverse::new(RemoveClipEffect::new(
            self.sequence,
            self.track,
            self.clip,
            id,
        )))
    }

    fn label(&self) -> String {
        "Add effect".to_owned()
    }
}

/// Puts an existing effect back into a clip's stack at a given index.
///
/// This is the inverse of [`RemoveClipEffect`], and therefore what redo runs
/// after an [`AddClipEffect`] is undone. It carries the whole effect —
/// identifier, enabled flag and bound values — so undo is exact.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsertClipEffect {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip the effect joins.
    pub clip: ClipId,
    /// Where in the stack it goes.
    pub index: usize,
    /// The effect itself, exactly as it was.
    pub effect: ClipEffect,
}

impl InsertClipEffect {
    /// Inserts `effect` into `clip`'s stack at `index`.
    #[must_use]
    pub fn new(
        sequence: SequenceId,
        track: TrackId,
        clip: ClipId,
        index: usize,
        effect: ClipEffect,
    ) -> Self {
        Self {
            sequence,
            track,
            clip,
            index,
            effect,
        }
    }
}

impl Command for InsertClipEffect {
    const KIND: &'static str = "clip.insert_effect";
    const DESCRIPTION: &'static str =
        "Insert an existing effect into a clip's effect stack at a given index.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let id = self.effect.id;
        let clip = clip_mut(project, self.sequence, self.track, self.clip)?;
        check_insert_index(self.index, clip.effects.len(), "effect")?;
        if clip.effects.iter().any(|effect| effect.id == id) {
            return Err(duplicate(self.sequence, self.track, self.clip, id));
        }

        let mut candidate = clip.clone();
        candidate.effects.insert(self.index, self.effect.clone());
        candidate.validate()?;
        *clip = candidate;
        Ok(Inverse::new(RemoveClipEffect::new(
            self.sequence,
            self.track,
            self.clip,
            id,
        )))
    }

    fn label(&self) -> String {
        format!("Restore effect {}", self.effect.plugin)
    }
}

/// Takes an effect off a clip's stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveClipEffect {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip the effect is applied to.
    pub clip: ClipId,
    /// The effect to remove.
    pub effect: EffectId,
}

impl RemoveClipEffect {
    /// Removes `effect` from `clip`'s stack.
    #[must_use]
    pub const fn new(sequence: SequenceId, track: TrackId, clip: ClipId, effect: EffectId) -> Self {
        Self {
            sequence,
            track,
            clip,
            effect,
        }
    }
}

impl Command for RemoveClipEffect {
    const KIND: &'static str = "clip.remove_effect";
    const DESCRIPTION: &'static str = "Remove an effect from a clip's effect stack.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let clip = clip_mut(project, self.sequence, self.track, self.clip)?;
        let index = index_of(clip, self.effect)
            .ok_or_else(|| not_found(self.sequence, self.track, self.clip, self.effect))?;
        let effect = clip.effects.remove(index);
        Ok(Inverse::new(InsertClipEffect::new(
            self.sequence,
            self.track,
            self.clip,
            index,
            effect,
        )))
    }

    fn label(&self) -> String {
        "Remove effect".to_owned()
    }
}

/// Moves an effect to another position in a clip's stack.
///
/// The index is the one the effect ends up at, counted in the stack *without*
/// the effect in it — the same counting [`super::ReorderTrack`] uses — so
/// moving the first of three effects to the end is `to_index` 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveClipEffect {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip the effect is applied to.
    pub clip: ClipId,
    /// The effect to move.
    pub effect: EffectId,
    /// The position it ends up at.
    pub to_index: usize,
}

impl MoveClipEffect {
    /// Moves `effect` to `to_index` in `clip`'s stack.
    #[must_use]
    pub const fn new(
        sequence: SequenceId,
        track: TrackId,
        clip: ClipId,
        effect: EffectId,
        to_index: usize,
    ) -> Self {
        Self {
            sequence,
            track,
            clip,
            effect,
            to_index,
        }
    }
}

impl Command for MoveClipEffect {
    const KIND: &'static str = "clip.move_effect";
    const DESCRIPTION: &'static str =
        "Move an effect to a different position in a clip's effect stack.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let clip = clip_mut(project, self.sequence, self.track, self.clip)?;
        let from = index_of(clip, self.effect)
            .ok_or_else(|| not_found(self.sequence, self.track, self.clip, self.effect))?;
        check_insert_index(self.to_index, clip.effects.len() - 1, "effect")?;

        if from != self.to_index {
            let effect = clip.effects.remove(from);
            clip.effects.insert(self.to_index, effect);
        }
        Ok(Inverse::new(Self::new(
            self.sequence,
            self.track,
            self.clip,
            self.effect,
            from,
        )))
    }

    fn label(&self) -> String {
        "Reorder effect".to_owned()
    }
}

/// Binds one parameter of one applied effect, or puts it back to the plugin's
/// declared default.
///
/// A `value` of `None` unbinds the parameter, which is how a control is reset:
/// the project stores only what differs from the declaration, so an unbound
/// parameter takes whatever the installed plugin declares. The inverse names
/// the same parameter carrying whatever was bound before — `None` included —
/// so undoing a slider drag leaves the other parameters of the effect alone.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetClipEffectParam {
    /// The sequence holding the track.
    pub sequence: SequenceId,
    /// The track holding the clip.
    pub track: TrackId,
    /// The clip the effect is applied to.
    pub clip: ClipId,
    /// The effect whose parameter is bound.
    pub effect: EffectId,
    /// The parameter id, as the plugin declared it.
    pub param: String,
    /// The value to bind, or `None` to take the plugin's declared default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<EffectValue>,
}

impl SetClipEffectParam {
    /// Binds `param` of `effect` to `value`.
    #[must_use]
    pub fn new(
        sequence: SequenceId,
        track: TrackId,
        clip: ClipId,
        effect: EffectId,
        param: impl Into<String>,
        value: EffectValue,
    ) -> Self {
        Self {
            sequence,
            track,
            clip,
            effect,
            param: param.into(),
            value: Some(value),
        }
    }

    /// Unbinds `param` of `effect`, putting it back to the declared default.
    #[must_use]
    pub fn cleared(
        sequence: SequenceId,
        track: TrackId,
        clip: ClipId,
        effect: EffectId,
        param: impl Into<String>,
    ) -> Self {
        Self {
            sequence,
            track,
            clip,
            effect,
            param: param.into(),
            value: None,
        }
    }
}

impl Command for SetClipEffectParam {
    const KIND: &'static str = "clip.set_effect_param";
    const DESCRIPTION: &'static str = "Bind one parameter of an effect applied to a clip, or clear it back to the plugin's \
         declared default.";

    fn apply(&self, project: &mut Project) -> SubResult<Inverse> {
        let clip = clip_mut(project, self.sequence, self.track, self.clip)?;
        let index = index_of(clip, self.effect)
            .ok_or_else(|| not_found(self.sequence, self.track, self.clip, self.effect))?;

        let mut candidate = clip.clone();
        let effect = &mut candidate.effects[index];
        let previous = match self.value {
            Some(value) => effect.params.insert(self.param.clone(), value),
            None => effect.params.remove(&self.param),
        };
        candidate.validate()?;
        *clip = candidate;

        Ok(Inverse::new(Self {
            sequence: self.sequence,
            track: self.track,
            clip: self.clip,
            effect: self.effect,
            param: self.param.clone(),
            value: previous,
        }))
    }

    fn label(&self) -> String {
        "Change effect parameter".to_owned()
    }
}

/// Where `effect` sits in `clip`'s stack, if it is in it at all.
fn index_of(clip: &Clip, effect: EffectId) -> Option<usize> {
    clip.effects.iter().position(|applied| applied.id == effect)
}

/// An `edit.effect_not_found` error naming what was looked for.
fn not_found(sequence: SequenceId, track: TrackId, clip: ClipId, effect: EffectId) -> SubError {
    SubError::new(codes::EFFECT_NOT_FOUND, "no such effect on the clip")
        .with_detail("sequence_id", sequence)
        .with_detail("track_id", track)
        .with_detail("clip_id", clip)
        .with_detail("effect_id", effect)
}

/// An `edit.duplicate_effect` error naming the identity that would repeat.
fn duplicate(sequence: SequenceId, track: TrackId, clip: ClipId, effect: EffectId) -> SubError {
    SubError::new(
        codes::DUPLICATE_EFFECT,
        "the clip already holds an effect with this identifier",
    )
    .with_detail("sequence_id", sequence)
    .with_detail("track_id", track)
    .with_detail("clip_id", clip)
    .with_detail("effect_id", effect)
}

#[cfg(test)]
mod tests {
    use super::{
        AddClipEffect, InsertClipEffect, MoveClipEffect, RemoveClipEffect, SetClipEffectParam,
    };
    use sub_model::effect::{ClipEffect, EffectValue};
    use sub_model::params::Fixed6;
    use sub_model::{
        Clip, ClipId, MediaItem, MediaPath, Project, Sequence, SequenceId, SequenceSettings, Track,
        TrackId, TrackKind,
    };
    use sub_time::{Rational, RationalTime, TimeRange};

    use crate::History;
    use crate::codes;

    /// A project with one clip on one video track.
    fn scene() -> (Project, SequenceId, TrackId, ClipId) {
        let mut project = Project::new("Doc cut");
        let media = MediaItem::new(MediaPath::new("a.mp4").expect("a valid path"));
        let media_id = media.id;
        project.media.push(media);

        let rate = Rational::FPS_24;
        let source = TimeRange::new(RationalTime::zero(rate), RationalTime::new(48, rate))
            .expect("a valid source range");
        let clip = Clip::new("shot 1", media_id, source);
        let clip_id = clip.id;
        let mut track = Track::new("V1", TrackKind::Video);
        track.items.push(clip.into());
        let track_id = track.id;
        let mut sequence = Sequence::new("Main", SequenceSettings::default());
        sequence.tracks.push(track);
        let sequence_id = sequence.id;
        project.sequences.push(sequence);
        (project, sequence_id, track_id, clip_id)
    }

    /// The clip's effect stack.
    fn effects(project: &Project, clip: ClipId) -> Vec<ClipEffect> {
        project.sequences[0].tracks[0]
            .clip(clip)
            .expect("the clip is still there")
            .effects
            .clone()
    }

    /// The plugin ids of the clip's effect stack, in order.
    fn plugins(project: &Project, clip: ClipId) -> Vec<String> {
        effects(project, clip)
            .into_iter()
            .map(|effect| effect.plugin)
            .collect()
    }

    #[test]
    fn adding_appends_to_the_stack_and_undoes_to_nothing() {
        let (mut project, sequence, track, clip) = scene();
        let mut history = History::new();
        for plugin in ["com.example.grade", "com.example.blur"] {
            history
                .apply(
                    &mut project,
                    AddClipEffect::new(sequence, track, clip, plugin),
                )
                .expect("the effect applies");
        }
        assert_eq!(
            plugins(&project, clip),
            vec!["com.example.grade", "com.example.blur"],
            "an effect joins the end of the stack"
        );

        history.undo(&mut project).expect("the add undoes");
        history.undo(&mut project).expect("and so does the first");
        assert!(plugins(&project, clip).is_empty());
    }

    #[test]
    fn a_redone_add_restores_the_same_identity_and_values() {
        let (mut project, sequence, track, clip) = scene();
        let mut history = History::new();
        history
            .apply(
                &mut project,
                AddClipEffect::new(sequence, track, clip, "com.example.grade"),
            )
            .expect("the effect applies");
        let id = effects(&project, clip)[0].id;
        history
            .apply(
                &mut project,
                SetClipEffectParam::new(
                    sequence,
                    track,
                    clip,
                    id,
                    "exposure",
                    EffectValue::Float(Fixed6::ONE),
                ),
            )
            .expect("the parameter binds");

        history.undo(&mut project).expect("the bind undoes");
        history.undo(&mut project).expect("the add undoes");
        history.redo(&mut project).expect("the add redoes");
        history.redo(&mut project).expect("the bind redoes");

        let restored = effects(&project, clip);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, id, "redo keeps the effect's identity");
        assert_eq!(
            restored[0].param("exposure"),
            Some(EffectValue::Float(Fixed6::ONE)),
            "and the value bound to it"
        );
    }

    #[test]
    fn removing_returns_the_whole_effect_at_its_index() {
        let (mut project, sequence, track, clip) = scene();
        let mut history = History::new();
        for plugin in ["com.example.grade", "com.example.blur"] {
            history
                .apply(
                    &mut project,
                    AddClipEffect::new(sequence, track, clip, plugin),
                )
                .expect("the effect applies");
        }
        let first = effects(&project, clip)[0].id;
        history
            .apply(
                &mut project,
                RemoveClipEffect::new(sequence, track, clip, first),
            )
            .expect("the effect comes off");
        assert_eq!(plugins(&project, clip), vec!["com.example.blur"]);

        history.undo(&mut project).expect("the removal undoes");
        let restored = effects(&project, clip);
        assert_eq!(
            restored.iter().map(|effect| effect.id).collect::<Vec<_>>(),
            vec![first, restored[1].id],
            "the effect goes back where it was, with its identity"
        );
    }

    #[test]
    fn moving_counts_the_index_without_the_effect_in_the_stack() {
        let (mut project, sequence, track, clip) = scene();
        let mut history = History::new();
        for plugin in [
            "com.example.grade",
            "com.example.blur",
            "com.example.sharpen",
        ] {
            history
                .apply(
                    &mut project,
                    AddClipEffect::new(sequence, track, clip, plugin),
                )
                .expect("the effect applies");
        }
        let first = effects(&project, clip)[0].id;
        history
            .apply(
                &mut project,
                MoveClipEffect::new(sequence, track, clip, first, 2),
            )
            .expect("the effect moves");
        assert_eq!(
            plugins(&project, clip),
            vec![
                "com.example.blur",
                "com.example.sharpen",
                "com.example.grade"
            ]
        );

        history.undo(&mut project).expect("the move undoes");
        assert_eq!(
            plugins(&project, clip),
            vec![
                "com.example.grade",
                "com.example.blur",
                "com.example.sharpen"
            ],
            "undo puts the effect back where it started"
        );
    }

    #[test]
    fn a_move_past_the_end_is_refused_with_a_code() {
        let (mut project, sequence, track, clip) = scene();
        let mut history = History::new();
        history
            .apply(
                &mut project,
                AddClipEffect::new(sequence, track, clip, "com.example.grade"),
            )
            .expect("the effect applies");
        let id = effects(&project, clip)[0].id;
        let error = history
            .apply(
                &mut project,
                MoveClipEffect::new(sequence, track, clip, id, 4),
            )
            .expect_err("an index past the end is refused");
        assert_eq!(error.code, codes::INVALID_INDEX);
    }

    #[test]
    fn an_unknown_effect_is_refused_with_a_code() {
        let (mut project, sequence, track, clip) = scene();
        let mut history = History::new();
        let stranger = ClipEffect::new("com.example.grade").expect("a valid plugin id");
        let error = history
            .apply(
                &mut project,
                RemoveClipEffect::new(sequence, track, clip, stranger.id),
            )
            .expect_err("an effect the clip does not hold is refused");
        assert_eq!(error.code, codes::EFFECT_NOT_FOUND);
    }

    #[test]
    fn an_effect_identity_cannot_appear_twice() {
        let (mut project, sequence, track, clip) = scene();
        let mut history = History::new();
        history
            .apply(
                &mut project,
                AddClipEffect::new(sequence, track, clip, "com.example.grade"),
            )
            .expect("the effect applies");
        let existing = effects(&project, clip)[0].clone();
        let error = history
            .apply(
                &mut project,
                InsertClipEffect::new(sequence, track, clip, 0, existing),
            )
            .expect_err("the same identity twice is refused");
        assert_eq!(error.code, codes::DUPLICATE_EFFECT);
    }

    #[test]
    fn a_bad_plugin_id_is_refused_before_anything_is_written() {
        let (mut project, sequence, track, clip) = scene();
        let mut history = History::new();
        let error = history
            .apply(
                &mut project,
                AddClipEffect::new(sequence, track, clip, "NotAPluginId"),
            )
            .expect_err("a malformed plugin id is refused");
        assert_eq!(error.code.as_str(), "model.invalid_effect");
        assert!(
            plugins(&project, clip).is_empty(),
            "and nothing was written"
        );
    }

    #[test]
    fn clearing_a_parameter_restores_the_value_it_had_on_undo() {
        let (mut project, sequence, track, clip) = scene();
        let mut history = History::new();
        history
            .apply(
                &mut project,
                AddClipEffect::new(sequence, track, clip, "com.example.grade"),
            )
            .expect("the effect applies");
        let id = effects(&project, clip)[0].id;
        history
            .apply(
                &mut project,
                SetClipEffectParam::new(sequence, track, clip, id, "exposure", EffectValue::Int(3)),
            )
            .expect("the parameter binds");
        history
            .apply(
                &mut project,
                SetClipEffectParam::cleared(sequence, track, clip, id, "exposure"),
            )
            .expect("the parameter clears");
        assert_eq!(
            effects(&project, clip)[0].param("exposure"),
            None,
            "a cleared parameter takes the plugin's default"
        );

        history.undo(&mut project).expect("the clear undoes");
        assert_eq!(
            effects(&project, clip)[0].param("exposure"),
            Some(EffectValue::Int(3)),
            "and the bound value comes back"
        );
    }

    #[test]
    fn a_locked_track_refuses_every_effect_edit() {
        let (mut project, sequence, track, clip) = scene();
        project.sequences[0].tracks[0].locked = true;
        let mut history = History::new();
        let error = history
            .apply(
                &mut project,
                AddClipEffect::new(sequence, track, clip, "com.example.grade"),
            )
            .expect_err("a locked track is refused");
        assert_eq!(error.code, codes::TRACK_LOCKED);
    }
}
