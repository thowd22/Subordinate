//! The central keyboard map: every action the editor exposes to the
//! keyboard, the chord each one answers to, and the help window that lists
//! them.
//!
//! Editors live on the keyboard, so bindings are declared once, here, rather
//! than being scattered over the panels that act on them (docs/PLAN.md §5.7).
//! One table means the help window is exhaustive by construction, a
//! double-bound chord is a startup warning instead of a mystery, and the
//! user-remappable config file of phase 5 has a single thing to override.
//!
//! An [`Action`] is *what* the user asked for; applying it belongs to the
//! panel that owns the state. Nothing here mutates the project, so nothing
//! here is a Command: [`ShortcutMap::poll`] only reports intent, and the
//! editing actions become undoable Commands where they are applied.
//!
//! ```
//! use eframe::egui::{Key, Modifiers};
//! use sub_ui::shortcuts::{Action, ShortcutMap};
//!
//! let map = ShortcutMap::default_map();
//! assert_eq!(map.action_for(Key::L, Modifiers::NONE), Some(Action::PlayForward));
//! assert_eq!(
//!     map.action_for(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT),
//!     Some(Action::Redo)
//! );
//! // Modifiers are matched exactly, so Ctrl+Z is undo and nothing else.
//! assert_eq!(map.action_for(Key::Z, Modifiers::NONE), None);
//! assert!(map.conflicts().is_empty());
//! ```

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};
use sub_core::{SubError, SubResult};

use crate::codes;

/// Where an action belongs in the help window, and roughly what it touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    /// Moving the playhead and running playback.
    Transport,
    /// In and out points and markers.
    Marking,
    /// Edits to the sequence, and undoing them.
    Editing,
    /// Editor state that is neither transport nor an edit.
    View,
}

impl Category {
    /// Every category, in the order the help window lists them.
    pub const ALL: [Self; 4] = [Self::Transport, Self::Marking, Self::Editing, Self::View];

    /// The heading shown for this category.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Transport => "Transport",
            Self::Marking => "Marking",
            Self::Editing => "Editing",
            Self::View => "View",
        }
    }
}

/// Something the user can ask the editor to do from the keyboard.
///
/// The variants are the contract: [`Action::id`] is stable, because the
/// remapping config file (phase 5) and plugin-registered shortcuts name
/// actions by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    /// Shuttle backwards, faster on each repeat (J).
    PlayBackward,
    /// Stop shuttling and hold the current frame (K).
    PausePlayback,
    /// Shuttle forwards, faster on each repeat (L).
    PlayForward,
    /// Play, or pause if already playing (Space).
    TogglePlayback,
    /// Move the playhead one frame back.
    StepBack,
    /// Move the playhead one frame forward.
    StepForward,
    /// Move the playhead to the first frame.
    GoToStart,
    /// Move the playhead to the last frame.
    GoToEnd,
    /// Mark an in point at the playhead (I).
    SetInPoint,
    /// Mark an out point at the playhead (O).
    SetOutPoint,
    /// Drop a marker at the playhead (M).
    AddMarker,
    /// Split the clips under the playhead (Ctrl+K).
    SplitAtPlayhead,
    /// Nudge the selection one frame earlier (comma).
    NudgeBack,
    /// Nudge the selection one frame later (period).
    NudgeForward,
    /// Undo the last command (Ctrl+Z).
    Undo,
    /// Redo the last undone command (Shift+Ctrl+Z).
    Redo,
    /// Turn timeline snapping on or off (S).
    ToggleSnapping,
    /// Show or hide the shortcut help window (F1).
    ShowShortcutHelp,
}

impl Action {
    /// Every action, in the order the help window lists them.
    pub const ALL: [Self; 18] = [
        Self::PlayBackward,
        Self::PausePlayback,
        Self::PlayForward,
        Self::TogglePlayback,
        Self::StepBack,
        Self::StepForward,
        Self::GoToStart,
        Self::GoToEnd,
        Self::SetInPoint,
        Self::SetOutPoint,
        Self::AddMarker,
        Self::SplitAtPlayhead,
        Self::NudgeBack,
        Self::NudgeForward,
        Self::Undo,
        Self::Redo,
        Self::ToggleSnapping,
        Self::ShowShortcutHelp,
    ];

    /// The stable identifier used by config files and plugin manifests.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::PlayBackward => "transport.play_backward",
            Self::PausePlayback => "transport.pause",
            Self::PlayForward => "transport.play_forward",
            Self::TogglePlayback => "transport.toggle_playback",
            Self::StepBack => "transport.step_back",
            Self::StepForward => "transport.step_forward",
            Self::GoToStart => "transport.go_to_start",
            Self::GoToEnd => "transport.go_to_end",
            Self::SetInPoint => "marking.set_in_point",
            Self::SetOutPoint => "marking.set_out_point",
            Self::AddMarker => "marking.add_marker",
            Self::SplitAtPlayhead => "editing.split_at_playhead",
            Self::NudgeBack => "editing.nudge_back",
            Self::NudgeForward => "editing.nudge_forward",
            Self::Undo => "editing.undo",
            Self::Redo => "editing.redo",
            Self::ToggleSnapping => "view.toggle_snapping",
            Self::ShowShortcutHelp => "view.show_shortcut_help",
        }
    }

    /// The human-readable name shown in menus and the help window.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::PlayBackward => "Play backward",
            Self::PausePlayback => "Pause",
            Self::PlayForward => "Play forward",
            Self::TogglePlayback => "Play / pause",
            Self::StepBack => "Step one frame back",
            Self::StepForward => "Step one frame forward",
            Self::GoToStart => "Go to start",
            Self::GoToEnd => "Go to end",
            Self::SetInPoint => "Set in point",
            Self::SetOutPoint => "Set out point",
            Self::AddMarker => "Add marker",
            Self::SplitAtPlayhead => "Split at playhead",
            Self::NudgeBack => "Nudge one frame back",
            Self::NudgeForward => "Nudge one frame forward",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::ToggleSnapping => "Toggle snapping",
            Self::ShowShortcutHelp => "Keyboard shortcuts",
        }
    }

    /// Which section of the help window lists this action.
    #[must_use]
    pub const fn category(self) -> Category {
        match self {
            Self::PlayBackward
            | Self::PausePlayback
            | Self::PlayForward
            | Self::TogglePlayback
            | Self::StepBack
            | Self::StepForward
            | Self::GoToStart
            | Self::GoToEnd => Category::Transport,
            Self::SetInPoint | Self::SetOutPoint | Self::AddMarker => Category::Marking,
            Self::SplitAtPlayhead
            | Self::NudgeBack
            | Self::NudgeForward
            | Self::Undo
            | Self::Redo => Category::Editing,
            Self::ToggleSnapping | Self::ShowShortcutHelp => Category::View,
        }
    }
}

/// One action and the chord that triggers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// What the chord asks for.
    pub action: Action,
    /// The chord itself.
    pub chord: KeyboardShortcut,
}

impl Binding {
    /// Binds `action` to `key` pressed with `modifiers`.
    #[must_use]
    pub const fn new(action: Action, modifiers: Modifiers, key: Key) -> Self {
        Self {
            action,
            chord: KeyboardShortcut::new(modifiers, key),
        }
    }

    /// The chord as text, e.g. `Ctrl+Z`, using this platform's modifier names.
    #[must_use]
    pub fn chord_label(&self) -> String {
        chord_label(self.chord)
    }
}

/// The chord as text, e.g. `Ctrl+Z` (`⇧⌘Z` on macOS).
#[must_use]
pub fn chord_label(chord: KeyboardShortcut) -> String {
    chord.format(&egui::ModifierNames::NAMES, cfg!(target_os = "macos"))
}

/// The bindings the editor ships with.
///
/// Ctrl is written as [`Modifiers::COMMAND`] so it is Cmd on macOS without a
/// second table. The set is the NLE convention: JKL shuttling, I/O for
/// in/out, comma and period for nudging, and the platform's own undo pair.
pub const DEFAULT_BINDINGS: &[Binding] = &[
    Binding::new(Action::PlayBackward, Modifiers::NONE, Key::J),
    Binding::new(Action::PausePlayback, Modifiers::NONE, Key::K),
    Binding::new(Action::PlayForward, Modifiers::NONE, Key::L),
    Binding::new(Action::TogglePlayback, Modifiers::NONE, Key::Space),
    Binding::new(Action::StepBack, Modifiers::NONE, Key::ArrowLeft),
    Binding::new(Action::StepForward, Modifiers::NONE, Key::ArrowRight),
    Binding::new(Action::GoToStart, Modifiers::NONE, Key::Home),
    Binding::new(Action::GoToEnd, Modifiers::NONE, Key::End),
    Binding::new(Action::SetInPoint, Modifiers::NONE, Key::I),
    Binding::new(Action::SetOutPoint, Modifiers::NONE, Key::O),
    Binding::new(Action::AddMarker, Modifiers::NONE, Key::M),
    Binding::new(Action::SplitAtPlayhead, Modifiers::COMMAND, Key::K),
    Binding::new(Action::NudgeBack, Modifiers::NONE, Key::Comma),
    Binding::new(Action::NudgeForward, Modifiers::NONE, Key::Period),
    Binding::new(Action::Undo, Modifiers::COMMAND, Key::Z),
    Binding::new(
        Action::Redo,
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
        Key::Z,
    ),
    Binding::new(Action::ToggleSnapping, Modifiers::NONE, Key::S),
    Binding::new(Action::ShowShortcutHelp, Modifiers::NONE, Key::F1),
];

/// The action the shipped map gives `key` pressed with exactly `modifiers`.
///
/// A borrow-free lookup into [`DEFAULT_BINDINGS`] for the panels that keep
/// their own key handling; the running editor should ask its [`ShortcutMap`],
/// which a user's remapping file can change.
#[must_use]
pub fn default_action_for(key: Key, modifiers: Modifiers) -> Option<Action> {
    action_in(DEFAULT_BINDINGS, key, modifiers)
}

/// The first binding in `bindings` matching `key` with exactly `modifiers`.
fn action_in(bindings: &[Binding], key: Key, modifiers: Modifiers) -> Option<Action> {
    bindings
        .iter()
        .find(|binding| {
            binding.chord.logical_key == key && modifiers.matches_exact(binding.chord.modifiers)
        })
        .map(|binding| binding.action)
}

/// One chord claimed by more than one action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The chord both actions want.
    pub chord: KeyboardShortcut,
    /// The actions bound to it, in table order; always two or more.
    pub actions: Vec<Action>,
}

impl Conflict {
    /// The one-line warning this conflict is logged as.
    #[must_use]
    pub fn message(&self) -> String {
        let actions: Vec<&str> = self.actions.iter().map(|action| action.id()).collect();
        format!(
            "{} is bound to {} actions: {}",
            chord_label(self.chord),
            self.actions.len(),
            actions.join(", ")
        )
    }
}

/// The editor's keyboard map: an ordered table of bindings.
///
/// Lookup is a linear scan of a table of a couple of dozen entries, which is
/// cheaper than hashing and keeps the declared order for the help window and
/// for conflict reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutMap {
    bindings: Vec<Binding>,
}

impl Default for ShortcutMap {
    fn default() -> Self {
        Self::default_map()
    }
}

impl ShortcutMap {
    /// The map the editor ships with, from [`DEFAULT_BINDINGS`].
    #[must_use]
    pub fn default_map() -> Self {
        Self {
            bindings: DEFAULT_BINDINGS.to_vec(),
        }
    }

    /// An empty map, for building one binding at a time.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    /// Every binding, in declaration order.
    #[must_use]
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// Adds a binding, keeping any binding already on the same chord.
    ///
    /// Overlaps are kept rather than silently dropped so [`Self::conflicts`]
    /// can report them; use [`Self::rebind`] to replace one instead.
    pub fn push(&mut self, binding: Binding) {
        self.bindings.push(binding);
    }

    /// Points `action` at `chord`, dropping every chord it had before.
    ///
    /// This is what a user's remapping file (phase 5) will call.
    pub fn rebind(&mut self, action: Action, chord: KeyboardShortcut) {
        self.bindings.retain(|binding| binding.action != action);
        self.bindings.push(Binding { action, chord });
    }

    /// The action `key` with exactly `modifiers` asks for, if any.
    ///
    /// Modifiers are matched exactly, so `Shift+Ctrl+Z` never falls through to
    /// the `Ctrl+Z` binding and a bare `S` is not triggered by `Shift+S`.
    #[must_use]
    pub fn action_for(&self, key: Key, modifiers: Modifiers) -> Option<Action> {
        action_in(&self.bindings, key, modifiers)
    }

    /// The first chord bound to `action`, if it has one.
    #[must_use]
    pub fn chord_for(&self, action: Action) -> Option<KeyboardShortcut> {
        self.bindings
            .iter()
            .find(|binding| binding.action == action)
            .map(|binding| binding.chord)
    }

    /// The chord bound to `action` as text, or `-` when it has none.
    #[must_use]
    pub fn chord_label_for(&self, action: Action) -> String {
        self.chord_for(action).map_or_else(
            || "-".to_owned(),
            |chord| chord.format(&egui::ModifierNames::NAMES, cfg!(target_os = "macos")),
        )
    }

    /// Every chord claimed by more than one action, in declaration order.
    #[must_use]
    pub fn conflicts(&self) -> Vec<Conflict> {
        let mut conflicts: Vec<Conflict> = Vec::new();
        for (index, binding) in self.bindings.iter().enumerate() {
            if self
                .bindings
                .iter()
                .take(index)
                .any(|earlier| earlier.chord == binding.chord)
            {
                // Already reported when its first holder was visited.
                continue;
            }
            let actions: Vec<Action> = self
                .bindings
                .iter()
                .filter(|other| other.chord == binding.chord)
                .map(|other| other.action)
                .collect();
            if actions.len() > 1 {
                conflicts.push(Conflict {
                    chord: binding.chord,
                    actions,
                });
            }
        }
        conflicts
    }

    /// Checks the map for double-bound chords.
    ///
    /// # Errors
    ///
    /// [`codes::SHORTCUT_CONFLICT`] when any chord is claimed by more than one
    /// action, with one detail per conflicting chord naming the action ids.
    pub fn validate(&self) -> SubResult<()> {
        let conflicts = self.conflicts();
        if conflicts.is_empty() {
            return Ok(());
        }
        let mut error = SubError::new(
            codes::SHORTCUT_CONFLICT,
            format!(
                "{} keyboard {} bound to more than one action",
                conflicts.len(),
                if conflicts.len() == 1 {
                    "shortcut is"
                } else {
                    "shortcuts are"
                }
            ),
        );
        for conflict in &conflicts {
            let actions: Vec<&str> = conflict.actions.iter().map(|action| action.id()).collect();
            error = error.with_detail(chord_label(conflict.chord), actions);
        }
        Err(error)
    }

    /// Logs every conflicting chord as a warning and returns how many there
    /// were.
    ///
    /// Called once at startup: a shortcut that silently does the wrong thing
    /// is worse than a noisy log line, and a conflict is a configuration
    /// problem rather than a reason to refuse to start.
    pub fn log_conflicts(&self) -> usize {
        let conflicts = self.conflicts();
        for conflict in &conflicts {
            log::warn!("keyboard shortcut conflict: {}", conflict.message());
        }
        if conflicts.is_empty() {
            log::debug!("{} keyboard shortcuts, no conflicts", self.bindings.len());
        }
        conflicts.len()
    }

    /// Takes the actions the pending key events ask for, consuming those
    /// events so no panel handles the same press twice.
    ///
    /// Returns nothing while a text field has the keyboard, so typing an `s`
    /// into a clip name cannot toggle snapping.
    #[must_use]
    pub fn poll(&self, ctx: &egui::Context) -> Vec<Action> {
        if ctx.egui_wants_keyboard_input() {
            return Vec::new();
        }
        ctx.input_mut(|input| self.take_actions(&mut input.events))
    }

    /// The map applied to a list of egui events: matched key presses are
    /// removed and reported, everything else is left alone.
    ///
    /// Key repeats count, so holding an arrow key steps repeatedly.
    #[must_use]
    pub fn take_actions(&self, events: &mut Vec<egui::Event>) -> Vec<Action> {
        let mut fired = Vec::new();
        events.retain(|event| {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            match self.action_for(*key, *modifiers) {
                Some(action) => {
                    fired.push(action);
                    false
                }
                None => true,
            }
        });
        fired
    }
}

/// One line of the shortcut help window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpRow {
    /// The section the row is listed under.
    pub category: Category,
    /// The action's human-readable name.
    pub label: &'static str,
    /// The chord, formatted for this platform.
    pub chord: String,
}

/// Every binding in `map` as help rows, grouped by category in
/// [`Category::ALL`] order and keeping the map's order within each group.
///
/// Every binding appears exactly once, so the help window cannot fall behind
/// the map.
#[must_use]
pub fn help_rows(map: &ShortcutMap) -> Vec<HelpRow> {
    let mut rows = Vec::with_capacity(map.bindings().len());
    for category in Category::ALL {
        rows.extend(
            map.bindings()
                .iter()
                .filter(|binding| binding.action.category() == category)
                .map(|binding| HelpRow {
                    category,
                    label: binding.action.label(),
                    chord: binding.chord_label(),
                }),
        );
    }
    rows
}

/// The window that lists every shortcut.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShortcutsWindow {
    /// Whether the window is showing.
    pub open: bool,
}

impl ShortcutsWindow {
    /// A closed window.
    #[must_use]
    pub const fn new() -> Self {
        Self { open: false }
    }

    /// Shows or hides the window.
    pub const fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Draws the window, if it is open.
    pub fn show(&mut self, ctx: &egui::Context, map: &ShortcutMap) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Keyboard shortcuts")
            .open(&mut open)
            .resizable(true)
            .default_width(360.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| Self::ui(ui, map));
            });
        self.open = open;
    }

    /// Draws the list into an existing layout.
    pub fn ui(ui: &mut egui::Ui, map: &ShortcutMap) {
        let mut current: Option<Category> = None;
        for row in help_rows(map) {
            if current != Some(row.category) {
                if current.is_some() {
                    ui.add_space(6.0);
                }
                ui.strong(row.category.label());
                current = Some(row.category);
            }
            ui.horizontal(|ui| {
                ui.monospace(&row.chord);
                ui.label(row.label);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Action, Binding, Category, DEFAULT_BINDINGS, ShortcutMap, ShortcutsWindow, help_rows,
    };
    use eframe::egui::{self, Key, Modifiers};
    use std::sync::Mutex;

    fn press(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn the_default_map_binds_every_action_exactly_once() {
        let map = ShortcutMap::default_map();
        assert_eq!(map.bindings().len(), Action::ALL.len());
        for action in Action::ALL {
            let bound = map
                .bindings()
                .iter()
                .filter(|binding| binding.action == action)
                .count();
            assert_eq!(bound, 1, "{} should be bound once", action.id());
        }
    }

    #[test]
    fn the_documented_editor_keys_are_bound() {
        let map = ShortcutMap::default_map();
        let expected = [
            (Key::J, Modifiers::NONE, Action::PlayBackward),
            (Key::K, Modifiers::NONE, Action::PausePlayback),
            (Key::L, Modifiers::NONE, Action::PlayForward),
            (Key::I, Modifiers::NONE, Action::SetInPoint),
            (Key::O, Modifiers::NONE, Action::SetOutPoint),
            (Key::Comma, Modifiers::NONE, Action::NudgeBack),
            (Key::Period, Modifiers::NONE, Action::NudgeForward),
            (Key::K, Modifiers::COMMAND, Action::SplitAtPlayhead),
            (Key::Z, Modifiers::COMMAND, Action::Undo),
            (
                Key::Z,
                Modifiers::COMMAND.plus(Modifiers::SHIFT),
                Action::Redo,
            ),
            (Key::S, Modifiers::NONE, Action::ToggleSnapping),
            (Key::M, Modifiers::NONE, Action::AddMarker),
            (Key::Space, Modifiers::NONE, Action::TogglePlayback),
        ];
        for (key, modifiers, action) in expected {
            assert_eq!(
                map.action_for(key, modifiers),
                Some(action),
                "{} should be bound to {}",
                super::chord_label(egui::KeyboardShortcut::new(modifiers, key)),
                action.id()
            );
        }
    }

    #[test]
    fn action_ids_and_labels_are_unique() {
        let mut ids: Vec<&str> = Action::ALL.iter().map(|action| action.id()).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "action ids must be unique");

        let mut labels: Vec<&str> = Action::ALL.iter().map(|action| action.label()).collect();
        labels.sort_unstable();
        let count = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), count, "action labels must be unique");
    }

    #[test]
    fn modifiers_are_matched_exactly() {
        let map = ShortcutMap::default_map();
        // Redo does not fall through to undo, and undo needs its modifier.
        assert_eq!(map.action_for(Key::Z, Modifiers::NONE), None);
        assert_eq!(
            map.action_for(Key::Z, Modifiers::COMMAND.plus(Modifiers::SHIFT)),
            Some(Action::Redo)
        );
        // A capital S is not the snapping toggle.
        assert_eq!(map.action_for(Key::S, Modifiers::SHIFT), None);
        assert_eq!(
            map.action_for(Key::S, Modifiers::NONE),
            Some(Action::ToggleSnapping)
        );
    }

    #[test]
    fn the_shipped_map_has_no_conflicts() {
        let map = ShortcutMap::default_map();
        assert_eq!(map.conflicts(), Vec::new());
        assert_eq!(map.log_conflicts(), 0);
        map.validate().expect("the shipped map should validate");
    }

    #[test]
    fn a_double_bound_chord_is_a_conflict_with_a_stable_code() {
        let mut map = ShortcutMap::default_map();
        // Bind marker-drop onto the snapping key without removing it.
        map.push(Binding::new(Action::AddMarker, Modifiers::NONE, Key::S));
        let conflicts = map.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(
            conflicts[0].actions,
            vec![Action::ToggleSnapping, Action::AddMarker],
            "conflicting actions are listed in table order"
        );
        assert!(conflicts[0].message().contains("view.toggle_snapping"));
        assert_eq!(map.log_conflicts(), 1);

        let err = map.validate().expect_err("a conflict should be an error");
        assert_eq!(err.code, crate::codes::SHORTCUT_CONFLICT);
        assert!(err.details.contains_key("S"), "details: {:?}", err.details);
    }

    #[test]
    fn rebinding_replaces_every_chord_an_action_had() {
        let mut map = ShortcutMap::default_map();
        map.rebind(
            Action::ToggleSnapping,
            egui::KeyboardShortcut::new(Modifiers::NONE, Key::N),
        );
        assert_eq!(map.action_for(Key::S, Modifiers::NONE), None);
        assert_eq!(
            map.action_for(Key::N, Modifiers::NONE),
            Some(Action::ToggleSnapping)
        );
        assert_eq!(map.bindings().len(), DEFAULT_BINDINGS.len());
        assert!(map.conflicts().is_empty());
    }

    #[test]
    fn polling_consumes_matched_presses_and_leaves_the_rest() {
        let map = ShortcutMap::default_map();
        let mut events = vec![
            press(Key::L, Modifiers::NONE),
            egui::Event::Text("q".to_owned()),
            press(Key::Q, Modifiers::NONE),
            press(Key::Z, Modifiers::COMMAND),
        ];
        let fired = map.take_actions(&mut events);
        assert_eq!(fired, vec![Action::PlayForward, Action::Undo]);
        assert_eq!(events.len(), 2, "unmatched events survive: {events:?}");
        assert!(matches!(events[0], egui::Event::Text(_)));
    }

    #[test]
    fn key_releases_are_not_actions() {
        let map = ShortcutMap::default_map();
        let mut events = vec![egui::Event::Key {
            key: Key::L,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        }];
        assert!(map.take_actions(&mut events).is_empty());
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn key_repeats_fire_again() {
        let map = ShortcutMap::default_map();
        let mut events = vec![
            press(Key::ArrowRight, Modifiers::NONE),
            egui::Event::Key {
                key: Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: Modifiers::NONE,
            },
        ];
        assert_eq!(
            map.take_actions(&mut events),
            vec![Action::StepForward, Action::StepForward]
        );
        assert!(events.is_empty());
    }

    #[test]
    fn help_rows_cover_every_binding_grouped_by_category() {
        let map = ShortcutMap::default_map();
        let rows = help_rows(&map);
        assert_eq!(rows.len(), map.bindings().len());
        for binding in map.bindings() {
            assert!(
                rows.iter()
                    .any(|row| row.label == binding.action.label()
                        && row.chord == binding.chord_label()),
                "{} is missing from the help window",
                binding.action.id()
            );
        }
        // Categories appear in one contiguous run each, in ALL order.
        let order: Vec<Category> = rows.iter().map(|row| row.category).collect();
        let mut seen: Vec<Category> = order.clone();
        seen.dedup();
        let expected: Vec<Category> = Category::ALL
            .into_iter()
            .filter(|category| order.contains(category))
            .collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn the_help_window_starts_closed_and_toggles() {
        let mut window = ShortcutsWindow::new();
        assert!(!window.open);
        window.toggle();
        assert!(window.open);
        window.toggle();
        assert!(!window.open);
    }

    /// Collects the text of every glyph run in a shape tree.
    fn collect_text(shape: &egui::Shape, into: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => into.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, into);
                }
            }
            _ => {}
        }
    }

    /// Paints `map`'s help list into a headless egui context and returns every
    /// string it actually drew. Two frames, because the first lays out fonts.
    fn painted_help(map: &ShortcutMap) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        for _ in 0..2 {
            texts.clear();
            let mut output =
                ctx.run_ui(egui::RawInput::default(), |ui| ShortcutsWindow::ui(ui, map));
            for clipped in &output.shapes {
                collect_text(&clipped.shape, &mut texts);
            }
            // Nothing here consumes the font atlas, so release it by hand
            // rather than let epaint panic on the unapplied delta.
            output.textures_delta.clear();
        }
        texts
    }

    #[test]
    fn the_painted_help_window_lists_every_binding() {
        let map = ShortcutMap::default_map();
        let painted = painted_help(&map);
        for binding in map.bindings() {
            let label = binding.action.label();
            assert!(
                painted.iter().any(|text| text == label),
                "{label} is missing from the painted help window: {painted:?}"
            );
            let chord = binding.chord_label();
            assert!(
                painted.iter().any(|text| text == chord.as_str()),
                "the chord {chord} is missing from the painted help window: {painted:?}"
            );
        }
        for category in Category::ALL {
            assert!(
                painted.iter().any(|text| text == category.label()),
                "the {} heading is missing: {painted:?}",
                category.label()
            );
        }
    }

    #[test]
    fn polling_a_context_fires_the_bound_actions_once() {
        let map = ShortcutMap::default_map();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![press(Key::M, Modifiers::NONE)],
            ..Default::default()
        };
        let mut fired = Vec::new();
        let mut twice = Vec::new();
        let mut output = ctx.run_ui(input, |ui| {
            fired = map.poll(ui.ctx());
            // A second poll in the same frame sees nothing: the first consumed
            // the event, so no other panel can act on it either.
            twice = map.poll(ui.ctx());
        });
        output.textures_delta.clear();
        assert_eq!(fired, vec![Action::AddMarker]);
        assert!(twice.is_empty(), "a consumed press fired twice: {twice:?}");
    }

    /// A `log` sink that keeps every warning, so the startup check can be
    /// shown to actually reach the log rather than only to count conflicts.
    struct CaptureLogger;

    static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    impl log::Log for CaptureLogger {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Warn
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata())
                && let Ok(mut captured) = CAPTURED.lock()
            {
                captured.push(record.args().to_string());
            }
        }

        fn flush(&self) {}
    }

    #[test]
    fn every_conflict_is_logged_as_a_warning() {
        // Another test binary in this process may own the logger already; the
        // check below is only meaningful when this one does.
        if log::set_logger(&CaptureLogger).is_err() {
            return;
        }
        log::set_max_level(log::LevelFilter::Warn);

        let mut map = ShortcutMap::default_map();
        map.push(Binding::new(Action::SetInPoint, Modifiers::NONE, Key::J));
        assert_eq!(map.log_conflicts(), 1);

        let captured = CAPTURED.lock().expect("the capture lock is never poisoned");
        assert!(
            captured.iter().any(|line| line.contains("conflict")
                && line.contains("marking.set_in_point")
                && line.contains("transport.play_backward")),
            "the conflict was not logged: {captured:?}"
        );
    }

    #[test]
    fn a_chord_label_names_its_modifiers() {
        let map = ShortcutMap::default_map();
        let label = map.chord_label_for(Action::Redo);
        assert!(label.contains('Z'), "label: {label}");
        assert!(label.contains("Shift"), "label: {label}");
        assert_eq!(map.chord_label_for(Action::PlayForward), "L");
    }
}
