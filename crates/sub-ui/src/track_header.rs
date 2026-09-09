//! The controls in a track header: name, mute, lock, and the menu that adds,
//! removes, renames and reorders tracks.
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
    Align, Button, Key, Label, Layout, Rect, Response, RichText, Sense, TextEdit, Ui, UiBuilder,
    Vec2, pos2,
};
use sub_edit::BoxedCommand;
use sub_edit::commands::{
    AddTrack, RemoveTrack, RenameTrack, ReorderTrack, SetTrackLocked, SetTrackMuted,
};
use sub_model::{SequenceId, Track, TrackId, TrackKind};

/// Padding between the header's edge and its controls, in points.
const PADDING: f32 = 5.0;

/// The size of the mute and lock toggles, in points.
const TOGGLE_SIZE: Vec2 = Vec2::new(20.0, 16.0);

/// The height of the row holding the kind label and the toggles, in points.
const CONTROL_ROW_HEIGHT: f32 = 16.0;

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
    /// The lock toggle.
    pub lock: Rect,
}

impl HeaderLayout {
    /// Splits `rect` into the name row and the control row.
    #[must_use]
    pub fn new(rect: Rect) -> Self {
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
            pos2(lock.left() - TOGGLE_SIZE.x - 2.0, controls_top),
            TOGGLE_SIZE,
        );
        Self {
            rect,
            name: Rect::from_min_max(inner.min, pos2(inner.right(), controls_top)),
            kind: Rect::from_min_max(
                pos2(inner.left(), controls_top),
                pos2((mute.left() - 2.0).max(inner.left()), inner.bottom()),
            ),
            mute,
            lock,
        }
    }
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
}

impl TrackHeaderState {
    /// A column with no rename in progress.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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
    ) -> Option<TrackAction> {
        let layout = HeaderLayout::new(rect);
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

        self.menu(&background, track, index, count).or(action)
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
fn select_all(ui: &Ui, id: eframe::egui::Id, text: &str) {
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
    fn the_controls_get_hit_targets_inside_the_header() {
        let layout = HeaderLayout::new(Rect::from_min_max(pos2(0.0, 0.0), pos2(132.0, 54.0)));
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
        let layout = HeaderLayout::new(Rect::from_min_max(pos2(0.0, 0.0), pos2(12.0, 8.0)));
        assert!(layout.name.height() >= 0.0 && layout.kind.height() >= 0.0);
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
