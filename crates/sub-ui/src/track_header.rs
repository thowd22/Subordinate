//! The controls in a track header: name, mute, solo, gain, lock, and the menu
//! that adds, removes, renames and reorders tracks.
//!
//! An audio lane carries two controls a video lane has no use for: a solo
//! toggle and a level in decibels. The level is dragged, so it behaves like
//! the inspector's sliders — every frame the number changes raises a command
//! the caller applies straight away, and the whole drag is one entry in the
//! undo stack because the outcome carries the group it opens and closes
//! ([`apply_actions`] is that caller for anyone who wants it done for them).
//!
//! The header column is the one part of the timeline that is made of widgets
//! rather than painted shapes. A sequence holds tens of tracks, not the
//! hundreds of clips the lanes hold, so the per-widget cost the lanes avoid
//! (docs/PLAN.md §5.7) is affordable here and buys real text editing, real
//! hover states and a real context menu.
//!
//! Nothing here mutates a project. Every control produces a [`TrackAction`],
//! which [`TrackAction::into_command`] turns into one of the track commands in
//! `sub-edit`; the caller applies it through the Command API, so every action
//! lands on the undo stack like any other edit.

use eframe::egui::text::CCursor;
use eframe::egui::text_selection::CCursorRange;
use eframe::egui::{
    Align, Button, DragValue, Key, Label, Layout, Rect, Response, RichText, Sense, TextEdit, Ui,
    UiBuilder, Vec2, pos2,
};
use sub_audio::MeterLevels;
use sub_core::SubResult;
use sub_edit::commands::{
    AddTrack, RemoveTrack, RenameTrack, ReorderTrack, SetTrackGain, SetTrackLocked, SetTrackMuted,
    SetTrackSolo,
};
use sub_edit::{BoxedCommand, History};
use sub_model::{GainDb, Project, SequenceId, Track, TrackId, TrackKind};

use crate::meter::MeterState;

use std::collections::HashMap;

/// Padding between the header's edge and its controls, in points.
const PADDING: f32 = 5.0;

/// The size of the mute, solo and lock toggles, in points.
const TOGGLE_SIZE: Vec2 = Vec2::new(20.0, 16.0);

/// The width of the gain field on an audio header, in points.
const GAIN_WIDTH: f32 = 52.0;

/// The height of the gain field on an audio header, in points.
const GAIN_HEIGHT: f32 = 18.0;

/// How many decibels a point of drag on the gain field is worth.
const GAIN_DRAG_SPEED: f64 = 0.2;

/// The label the one history entry a gain drag produces gets.
const GAIN_UNDO_LABEL: &str = "Change track gain";

/// The height of the row holding the kind label and the toggles, in points.
const CONTROL_ROW_HEIGHT: f32 = 16.0;

/// The height of the level meter in the control row, in points.
const METER_HEIGHT: f32 = 6.0;

/// The width the kind label is given before the meter takes the rest of the
/// control row, in points.
const KIND_WIDTH: f32 = 32.0;

/// The gap between the controls in the control row, in points.
const CONTROL_GAP: f32 = 2.0;

/// What a track header control asks the Command API to do.
///
/// Each variant maps onto exactly one command in
/// [`sub_edit::commands::track`], so the header has no editing logic of its
/// own and every action it raises is undoable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackAction {
    /// Add an empty track of `kind` at `index`.
    Add {
        /// Whether the new lane carries picture or sound.
        kind: TrackKind,
        /// Where in the stack it goes.
        index: usize,
    },
    /// Remove a track.
    Remove {
        /// The track to remove.
        track: TrackId,
        /// Whether to take its clips with it. A track that still holds clips
        /// is only removed when the menu offered that in so many words.
        force: bool,
    },
    /// Give a track a new name.
    Rename {
        /// The track to rename.
        track: TrackId,
        /// Its new name.
        name: String,
    },
    /// Move a track to another position in the stack.
    Reorder {
        /// The track to move.
        track: TrackId,
        /// The position it ends up at.
        to_index: usize,
    },
    /// Mute or unmute a track.
    SetMuted {
        /// The track to mute or unmute.
        track: TrackId,
        /// The state it ends up in.
        muted: bool,
    },
    /// Solo or unsolo a track.
    SetSolo {
        /// The track to solo or unsolo.
        track: TrackId,
        /// The state it ends up in.
        solo: bool,
    },
    /// Set a track's audio level.
    SetGain {
        /// The track whose level changes.
        track: TrackId,
        /// The level it ends up at.
        gain: GainDb,
    },
    /// Lock or unlock a track.
    SetLocked {
        /// The track to lock or unlock.
        track: TrackId,
        /// The state it ends up in.
        locked: bool,
    },
}

impl TrackAction {
    /// The command that performs this action on `sequence`.
    ///
    /// ```
    /// use sub_edit::{Command, History};
    /// use sub_model::{Project, Sequence, SequenceSettings, TrackKind};
    /// use sub_ui::track_header::TrackAction;
    ///
    /// let mut project = Project::new("Doc cut");
    /// let sequence = Sequence::new("Main", SequenceSettings::default());
    /// let sequence_id = sequence.id;
    /// project.sequences.push(sequence);
    ///
    /// let mut history = History::new();
    /// let action = TrackAction::Add { kind: TrackKind::Video, index: 0 };
    /// history
    ///     .apply_boxed(&mut project, action.into_command(sequence_id))
    ///     .unwrap();
    /// assert_eq!(project.sequences[0].tracks.len(), 1);
    ///
    /// history.undo(&mut project).unwrap();
    /// assert!(project.sequences[0].tracks.is_empty());
    /// ```
    #[must_use]
    pub fn into_command(self, sequence: SequenceId) -> BoxedCommand {
        match self {
            Self::Add { kind, index } => {
                Box::new(AddTrack::new(sequence, default_name(kind), kind).at(index))
            }
            Self::Remove { track, force } => Box::new(if force {
                RemoveTrack::forced(sequence, track)
            } else {
                RemoveTrack::new(sequence, track)
            }),
            Self::Rename { track, name } => Box::new(RenameTrack::new(sequence, track, name)),
            Self::Reorder { track, to_index } => {
                Box::new(ReorderTrack::new(sequence, track, to_index))
            }
            Self::SetMuted { track, muted } => Box::new(SetTrackMuted::new(sequence, track, muted)),
            Self::SetSolo { track, solo } => Box::new(SetTrackSolo::new(sequence, track, solo)),
            Self::SetGain { track, gain } => Box::new(SetTrackGain::new(sequence, track, gain)),
            Self::SetLocked { track, locked } => {
                Box::new(SetTrackLocked::new(sequence, track, locked))
            }
        }
    }
}

/// The name a track added from the header menu starts with.
///
/// It is a placeholder rather than a numbered lane name: the sequence may
/// already hold an `A2`, and renaming is one double-click away.
fn default_name(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "Video",
        TrackKind::Audio => "Audio",
    }
}

/// A short word for a track kind, shown under its name.
#[must_use]
pub const fn kind_label(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
    }
}

/// What choosing an entry in the header menu does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuChoice {
    /// Raise this action.
    Act(TrackAction),
    /// Open the inline editor over the track's name.
    BeginRename(TrackId),
}

/// One entry of the header's right-click menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    /// What the entry reads.
    pub label: String,
    /// What choosing it does.
    pub choice: MenuChoice,
}

impl MenuEntry {
    /// An entry labelled `label` that raises `action`.
    fn act(label: impl Into<String>, action: TrackAction) -> Self {
        Self {
            label: label.into(),
            choice: MenuChoice::Act(action),
        }
    }
}

/// The menu offered over the header of `track`, which sits at `index` of
/// `count` tracks.
///
/// Ordering is meaningful — a later video track composites over an earlier
/// one — so the move entries are only offered where there is somewhere to
/// move to, and a track that still holds clips says so in the entry that
/// removes it.
#[must_use]
pub fn menu_entries(track: &Track, index: usize, count: usize) -> Vec<MenuEntry> {
    let holds_clips = track.clips().next().is_some();
    let mut entries = vec![
        MenuEntry::act(
            "Add video track above",
            TrackAction::Add {
                kind: TrackKind::Video,
                index: index + 1,
            },
        ),
        MenuEntry::act(
            "Add audio track above",
            TrackAction::Add {
                kind: TrackKind::Audio,
                index: index + 1,
            },
        ),
        MenuEntry {
            label: "Rename track".to_owned(),
            choice: MenuChoice::BeginRename(track.id),
        },
    ];
    if index > 0 {
        entries.push(MenuEntry::act(
            "Move down",
            TrackAction::Reorder {
                track: track.id,
                to_index: index - 1,
            },
        ));
    }
    if index + 1 < count {
        entries.push(MenuEntry::act(
            "Move up",
            TrackAction::Reorder {
                track: track.id,
                to_index: index + 1,
            },
        ));
    }
    entries.push(MenuEntry::act(
        if holds_clips {
            "Remove track and its clips"
        } else {
            "Remove track"
        },
        TrackAction::Remove {
            track: track.id,
            force: holds_clips,
        },
    ));
    entries
}

/// The menu offered over the empty part of the header column, which can only
/// add a track at the top of the stack of `count` tracks.
#[must_use]
pub fn empty_menu_entries(count: usize) -> Vec<MenuEntry> {
    vec![
        MenuEntry::act(
            "Add video track",
            TrackAction::Add {
                kind: TrackKind::Video,
                index: count,
            },
        ),
        MenuEntry::act(
            "Add audio track",
            TrackAction::Add {
                kind: TrackKind::Audio,
                index: count,
            },
        ),
    ]
}

/// Where one header's controls sit inside the rectangle it was given.
///
/// The rectangles are computed rather than laid out so that the hit target of
/// every control is known without painting a frame, which is what lets the
/// header be tested headlessly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeaderLayout {
    /// The whole header.
    pub rect: Rect,
    /// The track name, or the editor that replaces it.
    pub name: Rect,
    /// The kind label, left of the toggles.
    pub kind: Rect,
    /// The mute toggle.
    pub mute: Rect,
    /// The solo toggle, which is empty on a video header.
    pub solo: Rect,
    /// The gain field, which is empty on a video header.
    pub gain: Rect,
    /// The lock toggle.
    pub lock: Rect,
    /// The level meter, between the kind label and the toggles.
    pub meter: Rect,
}

impl HeaderLayout {
    /// Splits `rect` into the name row and the control row for a `kind` lane.
    ///
    /// A video lane has no solo toggle and no gain field, so both come back
    /// empty and the name and the meter take the room they would have had.
    #[must_use]
    pub fn new(rect: Rect, kind: TrackKind) -> Self {
        // A header narrower or shorter than its own padding still has to
        // produce sane rectangles: the panel can be dragged to any size.
        let padding = PADDING
            .min(rect.width() / 2.0)
            .min(rect.height() / 2.0)
            .max(0.0);
        let inner = rect.shrink(padding);
        let controls_top = (inner.bottom() - CONTROL_ROW_HEIGHT).max(inner.top());
        let lock = Rect::from_min_size(
            pos2(inner.right() - TOGGLE_SIZE.x, controls_top),
            TOGGLE_SIZE,
        );
        let mute = Rect::from_min_size(
            pos2(lock.left() - TOGGLE_SIZE.x - CONTROL_GAP, controls_top),
            TOGGLE_SIZE,
        );
        // Sound is the only thing that can be soloed or levelled, so a video
        // header keeps the layout it had before the two controls existed.
        let audio = matches!(kind, TrackKind::Audio);
        let solo = if audio {
            Rect::from_min_size(
                pos2(mute.left() - TOGGLE_SIZE.x - CONTROL_GAP, controls_top),
                TOGGLE_SIZE,
            )
        } else {
            Rect::from_min_size(
                pos2(mute.left(), controls_top),
                Vec2::new(0.0, TOGGLE_SIZE.y),
            )
        };
        // The control row reads left to right: the kind, the level meter, then
        // the two toggles. The meter takes whatever the kind label leaves, and
        // closes to nothing rather than overlapping anything in a header too
        // narrow for all four.
        let kind_right = (inner.left() + KIND_WIDTH)
            .min(solo.left() - CONTROL_GAP)
            .max(inner.left());
        let kind = Rect::from_min_max(
            pos2(inner.left(), controls_top),
            pos2(kind_right, inner.bottom()),
        );
        let meter_height = METER_HEIGHT.min(inner.height());
        let meter_top = (controls_top + (CONTROL_ROW_HEIGHT - meter_height) / 2.0).max(inner.top());
        let meter_left = kind.right() + CONTROL_GAP;
        let meter = Rect::from_min_max(
            pos2(meter_left, meter_top),
            pos2(
                (solo.left() - CONTROL_GAP).max(meter_left),
                (meter_top + meter_height).min(inner.bottom()),
            ),
        );
        // The gain field sits at the right of the name row, where there is
        // room for a number wide enough to read.
        let name_bottom = controls_top;
        let gain_height = GAIN_HEIGHT.min((name_bottom - inner.top()).max(0.0));
        let gain_top = (inner.top() + ((name_bottom - inner.top()) - gain_height) / 2.0)
            .max(inner.top())
            .min(name_bottom);
        let gain_left = if audio {
            (inner.right() - GAIN_WIDTH).max(inner.left())
        } else {
            inner.right()
        };
        let gain = Rect::from_min_max(
            pos2(gain_left, gain_top),
            pos2(inner.right().max(gain_left), gain_top + gain_height),
        );
        let name_right = if audio {
            (gain.left() - CONTROL_GAP).max(inner.left())
        } else {
            inner.right()
        };
        Self {
            rect,
            name: Rect::from_min_max(inner.min, pos2(name_right, name_bottom)),
            kind,
            mute,
            solo,
            gain,
            lock,
            meter,
        }
    }
}

/// What one painted header row asked the Command API to do.
///
/// A click on a toggle is one action and nothing else; a gain drag is a run of
/// actions, one a frame, wrapped in the undo group named by
/// [`HeaderOutcome::begin`] and closed by [`HeaderOutcome::commit`], so the
/// whole gesture is one entry in the history however many frames it took.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderOutcome {
    /// The action the frame's input asked for, if any.
    pub action: Option<TrackAction>,
    /// The label of the undo group this frame opens, when a gesture begins.
    pub begin: Option<String>,
    /// Whether the gesture ended this frame, closing the group.
    pub commit: bool,
}

impl HeaderOutcome {
    /// True when the frame asked for nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.action.is_none() && self.begin.is_none() && !self.commit
    }
}

/// Applies what a painted frame of the header column raised, as one entry in
/// the undo stack per gesture.
///
/// # Errors
///
/// Returns whatever the history or a command refuses with; an unknown track
/// yields `edit.unknown_track`.
pub fn apply_actions(
    history: &mut History,
    project: &mut Project,
    sequence: SequenceId,
    outcome: &HeaderOutcome,
) -> SubResult<()> {
    if let Some(label) = &outcome.begin {
        history.begin_group(label.clone())?;
    }
    if let Some(action) = outcome.action.clone() {
        history.apply_boxed(project, action.into_command(sequence))?;
    }
    if outcome.commit {
        history.commit_group()?;
    }
    Ok(())
}

/// The rename in progress, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rename {
    /// The track being renamed.
    track: TrackId,
    /// What has been typed so far.
    name: String,
    /// Whether the editor has yet to be handed the keyboard, in which case
    /// the name it was seeded with is still selected whole.
    fresh: bool,
}

/// The header column's own state: the inline rename, and nothing else.
///
/// Everything else a header shows is read from the [`Track`] each frame, so
/// the column cannot drift out of step with the project.
#[derive(Debug, Default, Clone)]
pub struct TrackHeaderState {
    /// The rename in progress.
    rename: Option<Rename>,
    /// The track whose gain field a drag is open on, which is what tells a
    /// gesture still under the pointer from one that has just ended.
    gain_drag: Option<TrackId>,
    /// One meter per track that has been metered, keyed by track. A track
    /// with no entry draws a silent meter.
    meters: HashMap<TrackId, MeterState>,
}

impl TrackHeaderState {
    /// A column with no rename in progress.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The track whose gain is being dragged, if one is.
    #[must_use]
    pub const fn gain_drag(&self) -> Option<TrackId> {
        self.gain_drag
    }

    /// The track whose name is being edited, if one is.
    #[must_use]
    pub fn renaming(&self) -> Option<TrackId> {
        self.rename.as_ref().map(|rename| rename.track)
    }

    /// What has been typed into the rename editor so far.
    #[must_use]
    pub fn rename_text(&self) -> Option<&str> {
        self.rename.as_ref().map(|rename| rename.name.as_str())
    }

    /// Opens the rename editor over `track`, seeded with `name`.
    pub fn begin_rename(&mut self, track: TrackId, name: impl Into<String>) {
        self.rename = Some(Rename {
            track,
            name: name.into(),
            fresh: true,
        });
    }

    /// Closes the rename editor, discarding what was typed.
    pub fn cancel_rename(&mut self) {
        self.rename = None;
    }

    /// Closes the rename editor and returns the action that applies it.
    ///
    /// An empty or unchanged name is no edit at all, so it produces no action
    /// and therefore no history entry.
    pub fn commit_rename(&mut self, current: &str) -> Option<TrackAction> {
        let rename = self.rename.take()?;
        let name = rename.name.trim();
        if name.is_empty() || name == current {
            return None;
        }
        Some(TrackAction::Rename {
            track: rename.track,
            name: name.to_owned(),
        })
    }

    /// Applies a menu choice, returning the action it raises, if any.
    pub fn choose(&mut self, choice: MenuChoice, name: &str) -> Option<TrackAction> {
        match choice {
            MenuChoice::Act(action) => Some(action),
            MenuChoice::BeginRename(track) => {
                self.begin_rename(track, name);
                None
            }
        }
    }

    /// Feeds one track's meter with the levels the audio callback published,
    /// `elapsed` seconds after the last frame.
    ///
    /// Nothing is read from the audio thread here beyond the two atomics the
    /// caller already loaded: the header column only holds the peak and lets
    /// it fall.
    pub fn update_meter(&mut self, track: TrackId, levels: MeterLevels, elapsed: f32) {
        self.meters
            .entry(track)
            .or_default()
            .update(levels, elapsed);
    }

    /// Lets every meter fall by `elapsed` seconds without a new measurement,
    /// which is what a stopped transport does.
    pub fn decay_meters(&mut self, elapsed: f32) {
        for meter in self.meters.values_mut() {
            meter.decay(elapsed);
        }
    }

    /// Drops the meters of tracks that are no longer in the sequence.
    pub fn retain_meters(&mut self, keep: impl Fn(TrackId) -> bool) {
        self.meters.retain(|track, _| keep(*track));
    }

    /// The meter of `track`, when it has been fed one.
    #[must_use]
    pub fn meter(&self, track: TrackId) -> Option<&MeterState> {
        self.meters.get(&track)
    }

    /// Lays the controls of `track` out over `rect` and runs them.
    ///
    /// `index` is the track's position in a stack of `count` tracks, which is
    /// what the menu's move entries are built from. Returns the action the
    /// frame's input asked for, if any.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        track: &Track,
        index: usize,
        count: usize,
    ) -> HeaderOutcome {
        let layout = HeaderLayout::new(rect, track.kind);
        let mut outcome = HeaderOutcome::default();
        let mut action = None;
        let mut inner = ui.new_child(
            UiBuilder::new()
                .max_rect(layout.rect)
                .id_salt(("track_header", track.id))
                .layout(Layout::top_down(Align::Min)),
        );
        let background = inner.interact(
            layout.rect,
            inner.id().with("background"),
            Sense::click_and_drag(),
        );

        if self.renaming() == Some(track.id) {
            action = self
                .rename_editor(&mut inner, layout.name, track)
                .or(action);
        } else {
            let name = inner.put(
                layout.name,
                Label::new(RichText::new(&track.name).strong().size(12.0))
                    .truncate()
                    .sense(Sense::click()),
            );
            if name.double_clicked() {
                self.begin_rename(track.id, &track.name);
            }
            action = self.menu(&name, track, index, count).or(action);
        }

        inner.put(
            layout.kind,
            Label::new(RichText::new(kind_label(track.kind)).weak().size(10.0)).truncate(),
        );
        let mute = inner
            .put(
                layout.mute,
                Button::new(RichText::new("M").size(10.0)).selected(track.muted),
            )
            .on_hover_text(if track.muted {
                "Unmute this track"
            } else {
                "Mute this track"
            });
        if mute.clicked() {
            action = Some(TrackAction::SetMuted {
                track: track.id,
                muted: !track.muted,
            });
        }
        let lock = inner
            .put(
                layout.lock,
                Button::new(RichText::new("L").size(10.0)).selected(track.locked),
            )
            .on_hover_text(if track.locked {
                "Unlock this track for editing"
            } else {
                "Lock this track against edits"
            });
        if lock.clicked() {
            action = Some(TrackAction::SetLocked {
                track: track.id,
                locked: !track.locked,
            });
        }
        if matches!(track.kind, TrackKind::Audio) {
            let solo = inner
                .put(
                    layout.solo,
                    Button::new(RichText::new("S").size(10.0)).selected(track.solo),
                )
                .on_hover_text(if track.solo {
                    "Stop soloing this track"
                } else {
                    "Solo this track"
                });
            if solo.clicked() {
                action = Some(TrackAction::SetSolo {
                    track: track.id,
                    solo: !track.solo,
                });
            }
            if let Some(gain) = self.gain_field(&mut inner, layout.gain, track, &mut outcome) {
                action = Some(gain);
            }
        }

        // The meter is painted rather than laid out as a widget: it takes no
        // input, and the header's rectangles are computed up front so that the
        // column can be tested without a frame.
        let meter = self.meters.get(&track.id).copied().unwrap_or_default();
        meter.paint(inner.painter(), layout.meter, inner.visuals());

        outcome.action = self.menu(&background, track, index, count).or(action);
        outcome
    }

    /// The gain field of an audio header: a level in decibels, dragged.
    ///
    /// The number is a float because egui's drag value is, and it is turned
    /// back into a [`GainDb`] the same frame; nothing here keeps a float
    /// between frames. A value the model would refuse — the field is ranged,
    /// so only an overflow gets there — raises no action at all rather than a
    /// command that would fail.
    fn gain_field(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        track: &Track,
        outcome: &mut HeaderOutcome,
    ) -> Option<TrackAction> {
        let mut decibels = track.gain.decibels().as_f64();
        let painted = ui
            .put(
                rect,
                DragValue::new(&mut decibels)
                    .speed(GAIN_DRAG_SPEED)
                    .range(GainDb::MIN.as_f64()..=GainDb::MAX.as_f64())
                    .fixed_decimals(1)
                    .suffix(" dB"),
            )
            .on_hover_text("This track's level, in decibels");

        let mut action = None;
        if painted.changed() {
            if self.gain_drag.is_none() {
                self.gain_drag = Some(track.id);
                outcome.begin = Some(GAIN_UNDO_LABEL.to_owned());
            }
            action = GainDb::from_f64(decibels)
                .ok()
                .map(|gain| TrackAction::SetGain {
                    track: track.id,
                    gain,
                });
        }
        // The gesture ends when the pointer lets go, when the field loses
        // focus, or straight away when the change was never a drag: a typed
        // value, or an arrow key.
        let dragging = painted.dragged() || painted.drag_started();
        let released =
            painted.drag_stopped() || painted.lost_focus() || (painted.changed() && !dragging);
        if released && self.gain_drag == Some(track.id) {
            self.gain_drag = None;
            outcome.commit = true;
        }
        action
    }

    /// The inline name editor, which commits on Enter and on losing focus and
    /// abandons the edit on Escape.
    ///
    /// The editor opens with the old name selected, so the first keystroke
    /// replaces it rather than appending to it.
    fn rename_editor(&mut self, ui: &mut Ui, rect: Rect, track: &Track) -> Option<TrackAction> {
        let id = ui.id().with(("rename", track.id));
        let mut text = self
            .rename
            .as_ref()
            .map_or_else(String::new, |rename| rename.name.clone());
        let focused = ui.memory(|memory| memory.has_focus(id));
        if self.rename.as_ref().is_some_and(|rename| rename.fresh) {
            select_all(ui, id, &text);
            if focused && let Some(rename) = self.rename.as_mut() {
                rename.fresh = false;
            }
        }
        let response = ui.put(
            rect,
            TextEdit::singleline(&mut text)
                .id(id)
                .desired_width(rect.width())
                .font(eframe::egui::TextStyle::Small),
        );
        if let Some(rename) = self.rename.as_mut() {
            rename.name = text;
        }
        if !response.has_focus() && !response.lost_focus() {
            response.request_focus();
        }
        if ui.input(|input| input.key_pressed(Key::Escape)) {
            self.cancel_rename();
            return None;
        }
        let committed = response.lost_focus() || ui.input(|input| input.key_pressed(Key::Enter));
        if committed {
            return self.commit_rename(&track.name);
        }
        None
    }

    /// Attaches the header menu to `response`.
    fn menu(
        &mut self,
        response: &Response,
        track: &Track,
        index: usize,
        count: usize,
    ) -> Option<TrackAction> {
        let mut chosen = None;
        response.context_menu(|ui| {
            for entry in menu_entries(track, index, count) {
                if ui.button(&entry.label).clicked() {
                    chosen = Some(entry.choice);
                    ui.close();
                }
            }
        });
        chosen.and_then(|choice| self.choose(choice, &track.name))
    }
}

/// Selects the whole of a freshly opened rename editor.
///
/// Opening the editor over a name is a request to replace it, as it is
/// everywhere else a name is renamed in place, so the first keystroke should
/// not append to the old name.
pub(crate) fn select_all(ui: &Ui, id: eframe::egui::Id, text: &str) {
    let mut state = TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    let end = CCursor::new(text.chars().count());
    state
        .cursor
        .set_char_range(Some(CCursorRange::two(CCursor::new(0), end)));
    state.store(ui.ctx(), id);
}

/// Runs the menu offered over the empty part of the header column.
///
/// Returns the action it raised, if any.
pub fn empty_column_menu(response: &Response, count: usize) -> Option<TrackAction> {
    let mut chosen = None;
    response.context_menu(|ui| {
        for entry in empty_menu_entries(count) {
            if ui.button(&entry.label).clicked() {
                if let MenuChoice::Act(action) = entry.choice {
                    chosen = Some(action);
                }
                ui.close();
            }
        }
    });
    chosen
}

#[cfg(test)]
mod tests {
    use super::{
        HeaderLayout, MenuChoice, TrackAction, TrackHeaderState, empty_menu_entries, menu_entries,
    };
    use eframe::egui::{Rect, pos2};
    use sub_audio::MeterLevels;
    use sub_edit::History;
    use sub_model::sequence::SequenceSettings;
    use sub_model::{Project, Sequence, Track, TrackKind};

    fn track(name: &str) -> Track {
        Track::new(name, TrackKind::Video)
    }

    fn project_with(tracks: &[&str]) -> (Project, Sequence) {
        let mut project = Project::new("headers");
        let mut sequence = Sequence::new("edit", SequenceSettings::default());
        for name in tracks {
            sequence.tracks.push(track(name));
        }
        project.sequences.push(sequence.clone());
        (project, sequence)
    }

    #[test]
    fn the_meter_sits_in_the_control_row_between_the_kind_and_the_toggles() {
        let layout = HeaderLayout::new(
            Rect::from_min_max(pos2(0.0, 0.0), pos2(132.0, 54.0)),
            TrackKind::Video,
        );
        assert!(
            layout.rect.contains_rect(layout.meter),
            "the meter escapes the header"
        );
        assert!(layout.meter.width() > 0.0 && layout.meter.height() > 0.0);
        assert!(
            layout.meter.left() >= layout.kind.right(),
            "the meter starts after the kind label"
        );
        assert!(
            layout.meter.right() <= layout.mute.left(),
            "the meter stops before the toggles"
        );
        assert!(
            layout.meter.top() >= layout.name.bottom(),
            "the meter sits below the name, in the control row"
        );
    }

    #[test]
    fn a_header_too_narrow_for_a_meter_closes_it_to_nothing() {
        // The toggles alone are wider than this header, so there is no room
        // left for a meter at all: it must come out empty rather than
        // negative, since a painter would draw an inverted rectangle.
        let layout = HeaderLayout::new(
            Rect::from_min_max(pos2(0.0, 0.0), pos2(40.0, 54.0)),
            TrackKind::Video,
        );
        assert!(
            layout.meter.width() <= 0.0 + f32::EPSILON,
            "a squeezed meter is empty, not {} wide",
            layout.meter.width()
        );
        assert!(layout.meter.height() >= 0.0);
    }

    #[test]
    fn a_meter_holds_its_peak_per_track_and_can_be_dropped_with_its_track() {
        let (_, sequence) = project_with(&["V1", "V2"]);
        let first = sequence.tracks[0].id;
        let second = sequence.tracks[1].id;
        let mut state = TrackHeaderState::new();
        assert!(state.meter(first).is_none(), "an unfed track has no meter");

        state.update_meter(first, MeterLevels::new(1.0, 0.5), 1.0 / 60.0);
        let meter = state.meter(first).expect("a meter");
        assert!(meter.clipping(), "full scale lights the clip indicator");
        assert!((meter.peak_hold_db() - 0.0).abs() < 1e-6);
        assert!(state.meter(second).is_none(), "meters do not bleed across");

        state.decay_meters(10.0);
        assert!(
            !state.meter(first).expect("a meter").clipping(),
            "the indicator clears once the transport has been quiet"
        );

        state.retain_meters(|track| track == second);
        assert!(state.meter(first).is_none(), "a removed track is forgotten");
    }

    #[test]
    fn the_controls_get_hit_targets_inside_the_header() {
        let layout = HeaderLayout::new(
            Rect::from_min_max(pos2(0.0, 0.0), pos2(132.0, 54.0)),
            TrackKind::Video,
        );
        for control in [layout.name, layout.kind, layout.mute, layout.lock] {
            assert!(
                layout.rect.contains_rect(control),
                "{control:?} escapes the header {:?}",
                layout.rect
            );
            assert!(control.width() > 0.0 && control.height() > 0.0);
        }
        assert!(
            layout.mute.right() <= layout.lock.left(),
            "the toggles must not overlap"
        );
        assert!(
            layout.kind.right() <= layout.mute.left(),
            "the kind label stops at the toggles"
        );
        assert!(
            layout.name.bottom() <= layout.mute.top(),
            "the name sits above the controls"
        );
    }

    #[test]
    fn a_header_that_is_too_small_still_lays_out() {
        let layout = HeaderLayout::new(
            Rect::from_min_max(pos2(0.0, 0.0), pos2(12.0, 8.0)),
            TrackKind::Audio,
        );
        assert!(layout.name.height() >= 0.0 && layout.kind.height() >= 0.0);
    }

    #[test]
    fn an_audio_header_lays_the_solo_toggle_and_the_gain_field_out_too() {
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(132.0, 54.0));
        let audio = HeaderLayout::new(rect, TrackKind::Audio);
        for control in [audio.solo, audio.gain] {
            assert!(
                audio.rect.contains_rect(control),
                "{control:?} escapes the header {:?}",
                audio.rect
            );
            assert!(control.width() > 0.0 && control.height() > 0.0);
        }
        assert!(
            audio.solo.right() <= audio.mute.left(),
            "the toggles must not overlap"
        );
        assert!(
            audio.meter.right() <= audio.solo.left(),
            "the meter stops before the solo toggle"
        );
        assert!(
            audio.name.right() <= audio.gain.left(),
            "the name stops before the gain field"
        );
        assert!(
            audio.gain.bottom() <= audio.mute.top() + f32::EPSILON,
            "the gain field sits in the name row"
        );

        let video = HeaderLayout::new(rect, TrackKind::Video);
        assert!(
            video.solo.width() <= f32::EPSILON && video.gain.width() <= f32::EPSILON,
            "a video header carries neither control"
        );
        assert!(
            video.meter.width() > audio.meter.width(),
            "and gives the room back to the meter"
        );
    }

    #[test]
    fn the_menu_offers_only_the_moves_that_exist() {
        let first = track("V1");
        let entries = menu_entries(&first, 0, 3);
        let labels: Vec<&str> = entries.iter().map(|entry| entry.label.as_str()).collect();
        assert!(labels.contains(&"Move up"), "{labels:?}");
        assert!(!labels.contains(&"Move down"), "{labels:?}");
        assert!(labels.iter().any(|label| label.starts_with("Add video")));
        assert!(labels.contains(&"Rename track"));
        assert!(labels.contains(&"Remove track"));

        let only = menu_entries(&first, 0, 1);
        let labels: Vec<&str> = only.iter().map(|entry| entry.label.as_str()).collect();
        assert!(!labels.contains(&"Move up"), "{labels:?}");
        assert!(!labels.contains(&"Move down"), "{labels:?}");
    }

    #[test]
    fn removing_a_track_that_holds_clips_is_forced_and_says_so() {
        let (_, sequence) = project_with(&["V1"]);
        let empty = &sequence.tracks[0];
        let entries = menu_entries(empty, 0, 1);
        let remove = entries.last().expect("a remove entry");
        assert_eq!(remove.label, "Remove track");
        assert_eq!(
            remove.choice,
            MenuChoice::Act(TrackAction::Remove {
                track: empty.id,
                force: false
            })
        );
    }

    #[test]
    fn the_empty_column_menu_appends() {
        let entries = empty_menu_entries(2);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].choice,
            MenuChoice::Act(TrackAction::Add {
                kind: TrackKind::Video,
                index: 2
            })
        );
    }

    #[test]
    fn a_rename_commits_once_and_ignores_an_empty_or_unchanged_name() {
        let (_, sequence) = project_with(&["V1"]);
        let id = sequence.tracks[0].id;
        let mut state = TrackHeaderState::new();

        state.begin_rename(id, "V1");
        assert_eq!(state.renaming(), Some(id));
        assert_eq!(state.rename_text(), Some("V1"));
        assert!(
            state.commit_rename("V1").is_none(),
            "no name change is no edit"
        );
        assert_eq!(state.renaming(), None, "committing closes the editor");

        state.begin_rename(id, "   ");
        assert!(
            state.commit_rename("V1").is_none(),
            "an empty name is no edit"
        );

        state.begin_rename(id, " Dialogue ");
        assert_eq!(
            state.commit_rename("V1"),
            Some(TrackAction::Rename {
                track: id,
                name: "Dialogue".to_owned()
            }),
            "the typed name is trimmed"
        );
    }

    #[test]
    fn escaping_a_rename_raises_nothing() {
        let (_, sequence) = project_with(&["V1"]);
        let mut state = TrackHeaderState::new();
        state.begin_rename(sequence.tracks[0].id, "Dialogue");
        state.cancel_rename();
        assert_eq!(state.renaming(), None);
        assert!(state.commit_rename("V1").is_none());
    }

    #[test]
    fn choosing_rename_opens_the_editor_instead_of_raising_an_action() {
        let (_, sequence) = project_with(&["V1"]);
        let id = sequence.tracks[0].id;
        let mut state = TrackHeaderState::new();
        assert!(state.choose(MenuChoice::BeginRename(id), "V1").is_none());
        assert_eq!(state.renaming(), Some(id));
    }

    #[test]
    fn every_action_applies_and_undoes_through_the_history() {
        let (mut project, sequence) = project_with(&["V1", "V2"]);
        let sequence_id = sequence.id;
        let first = project.sequences[0].tracks[0].id;
        let mut history = History::new();

        let actions = [
            TrackAction::Add {
                kind: TrackKind::Audio,
                index: 2,
            },
            TrackAction::SetMuted {
                track: first,
                muted: true,
            },
            TrackAction::SetLocked {
                track: first,
                locked: true,
            },
            TrackAction::Rename {
                track: first,
                name: "Dialogue".to_owned(),
            },
            TrackAction::Reorder {
                track: first,
                to_index: 1,
            },
            TrackAction::Remove {
                track: first,
                force: false,
            },
        ];

        for action in actions {
            let before = project.clone();
            history
                .apply_boxed(&mut project, action.clone().into_command(sequence_id))
                .unwrap_or_else(|err| panic!("{action:?} should apply: {err}"));
            assert_ne!(
                project.sequences[0].tracks, before.sequences[0].tracks,
                "{action:?} should change the sequence"
            );
            history.undo(&mut project).expect("undo");
            assert_eq!(
                project.sequences[0].tracks, before.sequences[0].tracks,
                "{action:?} should undo exactly"
            );
            history.redo(&mut project).expect("redo");
            history.undo(&mut project).expect("undo again");
            assert_eq!(project.sequences[0].tracks, before.sequences[0].tracks);
        }
    }
}
