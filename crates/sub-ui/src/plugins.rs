//! The Plugins menu and the plugin half of the shortcut registry.
//!
//! A `commands`-world plugin declares what it contributes and, optionally, the
//! chord it would like (docs/PLAN.md §6.2). The host validates that in
//! [`sub_plugin::menu`]; what is left is the editor's half of the bargain:
//! every registered command gets an entry in the Plugins menu, and the ones
//! whose chord is free get a place in the keyboard map.
//!
//! Chords are requests, not claims. The editor's own actions and the user's
//! `keymap.toml` are settled before a plugin is asked anything, so a plugin
//! asking for Ctrl+Z is refused rather than shadowing undo, and two plugins
//! asking for the same chord are resolved first come, first served. A refused
//! chord costs the command nothing but its shortcut: it is still in the menu,
//! and the reason is a [`SubError`] with a stable code that the diagnostics
//! panel and the log can show.
//!
//! ```
//! use eframe::egui::{Key, Modifiers};
//! use sub_plugin::menu::{CommandDesc, PluginCommandRegistry};
//! use sub_ui::plugins::PluginMenu;
//! use sub_ui::shortcuts::ShortcutMap;
//!
//! let mut registry = PluginCommandRegistry::new();
//! registry
//!     .register(
//!         "com.example.silence-cutter",
//!         &[CommandDesc {
//!             id: "cut-silence".to_owned(),
//!             title: "Cut silence".to_owned(),
//!             shortcut: Some("Ctrl+Shift+K".to_owned()),
//!         }],
//!     )
//!     .unwrap();
//!
//! let menu = PluginMenu::register(&registry, &ShortcutMap::default_map());
//! assert_eq!(menu.entries().len(), 1);
//! assert_eq!(
//!     menu.command_for(Key::K, Modifiers::COMMAND | Modifiers::SHIFT),
//!     Some("com.example.silence-cutter/cut-silence")
//! );
//! assert!(menu.problems().is_empty());
//! ```

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, Ui};
use sub_core::SubError;
use sub_plugin::menu::{PluginCommand, PluginCommandRegistry};

use crate::codes;
use crate::keymap::parse_chord;
use crate::shortcuts::{ShortcutMap, chord_label};

/// The title of the menu plugin commands appear under.
pub const MENU_TITLE: &str = "Plugins";

/// What the menu shows when no plugin has contributed a command.
pub const EMPTY_LABEL: &str = "No plugin commands";

/// One plugin command as the menu and the keyboard see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMenuEntry {
    /// `<plugin id>/<command id>`: what running the command asks for.
    pub qualified_id: String,
    /// The plugin that contributed it, for grouping the menu.
    pub plugin: String,
    /// The name shown in the menu.
    pub title: String,
    /// The chord it ended up with, or `None` when it asked for none or its
    /// request was refused.
    pub chord: Option<KeyboardShortcut>,
}

impl PluginMenuEntry {
    /// The chord as text, or `-` when the command has none.
    #[must_use]
    pub fn chord_label(&self) -> String {
        self.chord.map_or_else(|| "-".to_owned(), chord_label)
    }
}

/// The Plugins menu and the plugin commands' place in the keyboard map.
///
/// Rebuilt whenever the registry changes — a plugin installed, reloaded or
/// removed — and whenever the user's keymap changes, since the host map is
/// what plugin chords are checked against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginMenu {
    entries: Vec<PluginMenuEntry>,
    problems: Vec<SubError>,
}

impl PluginMenu {
    /// A menu with no plugin commands in it.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Puts every registered command in the menu, and every command whose
    /// chord is free in the keyboard map.
    ///
    /// `shortcuts` is the editor's own map, already overlaid with the user's
    /// keymap: a chord it claims is not available to a plugin.
    #[must_use]
    pub fn register(registry: &PluginCommandRegistry, shortcuts: &ShortcutMap) -> Self {
        let mut menu = Self::empty();
        for command in registry.commands() {
            let chord = menu.claim(command, shortcuts);
            menu.entries.push(PluginMenuEntry {
                qualified_id: command.qualified_id().to_owned(),
                plugin: command.plugin().to_owned(),
                title: command.title().to_owned(),
                chord,
            });
        }
        menu
    }

    /// The chord `command` may have, recording why when it may have none.
    fn claim(
        &mut self,
        command: &PluginCommand,
        shortcuts: &ShortcutMap,
    ) -> Option<KeyboardShortcut> {
        let requested = command.shortcut()?;
        let chord = match parse_chord(requested) {
            Ok(chord) => chord,
            Err(error) => {
                self.problems.push(
                    SubError::new(
                        codes::PLUGIN_INVALID_CHORD,
                        format!(
                            "plugin command {} asked for {requested:?}, which is not a chord",
                            command.qualified_id()
                        ),
                    )
                    .with_detail("command", command.qualified_id())
                    .with_detail("chord", requested)
                    .with_detail("reason", error.message),
                );
                return None;
            }
        };

        if let Some(action) = shortcuts.action_for(chord.logical_key, chord.modifiers) {
            let problem = conflict(command, chord).with_detail("claimed_by", action.id());
            self.problems.push(problem);
            return None;
        }
        if let Some(earlier) = self.entry_for(chord) {
            let problem = conflict(command, chord).with_detail("claimed_by", earlier);
            self.problems.push(problem);
            return None;
        }
        Some(chord)
    }

    /// The qualified id of the entry already holding `chord`, if any.
    fn entry_for(&self, chord: KeyboardShortcut) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.chord == Some(chord))
            .map(|entry| entry.qualified_id.as_str())
    }

    /// Every entry, in registry order.
    #[must_use]
    pub fn entries(&self) -> &[PluginMenuEntry] {
        &self.entries
    }

    /// Every chord that could not be given out, and why.
    #[must_use]
    pub fn problems(&self) -> &[SubError] {
        &self.problems
    }

    /// Whether no plugin has contributed a command.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The chord bound to `qualified_id`, if it has one.
    #[must_use]
    pub fn chord_for(&self, qualified_id: &str) -> Option<KeyboardShortcut> {
        self.entries
            .iter()
            .find(|entry| entry.qualified_id == qualified_id)
            .and_then(|entry| entry.chord)
    }

    /// The command `key` with exactly `modifiers` asks for, if any.
    ///
    /// Modifiers are matched exactly, the same rule the editor's own map
    /// follows, so `Shift+Ctrl+K` never falls through to a `Ctrl+K` binding.
    #[must_use]
    pub fn command_for(&self, key: Key, modifiers: Modifiers) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| {
                entry.chord.is_some_and(|chord| {
                    chord.logical_key == key && modifiers.matches_exact(chord.modifiers)
                })
            })
            .map(|entry| entry.qualified_id.as_str())
    }

    /// Logs every refused chord as a warning and returns how many there were.
    ///
    /// Called when the menu is rebuilt: a plugin whose shortcut silently did
    /// nothing is a bug report, so the reason goes in the log.
    pub fn log_problems(&self) -> usize {
        for problem in &self.problems {
            log::warn!("plugin shortcut: {}", problem.message);
        }
        self.problems.len()
    }

    /// Takes the plugin commands the pending key events ask for, consuming
    /// those events.
    ///
    /// Call it after [`ShortcutMap::poll`](crate::shortcuts::ShortcutMap::poll)
    /// so the editor's own actions get first refusal, and never while a text
    /// field has the keyboard.
    #[must_use]
    pub fn poll(&self, ctx: &egui::Context) -> Vec<String> {
        if self.entries.is_empty() || ctx.egui_wants_keyboard_input() {
            return Vec::new();
        }
        ctx.input_mut(|input| self.take_commands(&mut input.events))
    }

    /// The menu applied to a list of egui events: matched key presses are
    /// removed and reported as qualified command ids.
    #[must_use]
    pub fn take_commands(&self, events: &mut Vec<egui::Event>) -> Vec<String> {
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
            match self.command_for(*key, *modifiers) {
                Some(id) => {
                    fired.push(id.to_owned());
                    false
                }
                None => true,
            }
        });
        fired
    }
}

/// The error a refused chord is reported as.
///
/// `claimed_by` is filled in by the caller with whatever already holds the
/// chord: an editor action's id, or an earlier plugin command's qualified id.
fn conflict(command: &PluginCommand, chord: KeyboardShortcut) -> SubError {
    SubError::new(
        codes::PLUGIN_SHORTCUT_CONFLICT,
        format!(
            "plugin command {} asked for {}, which is already taken",
            command.qualified_id(),
            chord_label(chord)
        ),
    )
    .with_detail("command", command.qualified_id())
    .with_detail("chord", chord_label(chord))
}

/// Draws the Plugins menu, one entry per registered command with its chord on
/// the right, grouped by the plugin that contributed it.
///
/// Returns the qualified id of the command the user chose, if any. A menu over
/// an editor with no plugins offers a single disabled line rather than an empty
/// popup, so the user can tell "none installed" from "broken".
pub fn plugins_menu_ui(ui: &mut Ui, menu: &PluginMenu) -> Option<String> {
    let mut chosen = None;
    ui.menu_button(MENU_TITLE, |ui| {
        if menu.is_empty() {
            ui.add_enabled(false, egui::Button::new(EMPTY_LABEL));
            return;
        }
        let mut plugin: Option<&str> = None;
        for entry in menu.entries() {
            if plugin.is_some_and(|previous| previous != entry.plugin) {
                ui.separator();
            }
            plugin = Some(&entry.plugin);
            let button = egui::Button::new(&entry.title).shortcut_text(entry.chord_label());
            if ui.add(button).clicked() {
                chosen = Some(entry.qualified_id.clone());
                ui.close();
            }
        }
    });
    chosen
}

#[cfg(test)]
mod tests {
    use eframe::egui::{Key, Modifiers};
    use sub_plugin::menu::{CommandDesc, PluginCommandRegistry};

    use super::PluginMenu;
    use crate::shortcuts::ShortcutMap;

    fn desc(id: &str, title: &str, shortcut: Option<&str>) -> CommandDesc {
        CommandDesc {
            id: id.to_owned(),
            title: title.to_owned(),
            shortcut: shortcut.map(str::to_owned),
        }
    }

    /// How `Ctrl+Shift+G` reads back once parsed: `Ctrl` is `Modifiers::COMMAND`,
    /// which prints as `Cmd` in macOS order.
    const SHIFT_G_LABEL: &str = if cfg!(target_os = "macos") {
        "Shift+Cmd+G"
    } else {
        "Ctrl+Shift+G"
    };

    fn registry(plugin: &str, descs: &[CommandDesc]) -> PluginCommandRegistry {
        let mut registry = PluginCommandRegistry::new();
        registry
            .register(plugin, descs)
            .expect("valid descriptions");
        registry
    }

    #[test]
    fn a_free_chord_is_granted_and_fires_its_command() {
        let registry = registry("p", &[desc("go", "Go", Some("Ctrl+Shift+G"))]);
        let menu = PluginMenu::register(&registry, &ShortcutMap::default_map());

        assert!(menu.problems().is_empty());
        assert_eq!(menu.entries()[0].chord_label(), SHIFT_G_LABEL);
        let mut events = vec![eframe::egui::Event::Key {
            key: Key::G,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
        }];
        assert_eq!(menu.take_commands(&mut events), vec!["p/go".to_owned()]);
        assert!(events.is_empty(), "the event was consumed");
    }

    #[test]
    fn a_chord_the_editor_already_owns_is_refused_and_the_command_stays_in_the_menu() {
        let registry = registry("p", &[desc("undo-ish", "Undo-ish", Some("Ctrl+Z"))]);
        let menu = PluginMenu::register(&registry, &ShortcutMap::default_map());

        assert_eq!(menu.entries().len(), 1, "the command is still in the menu");
        assert_eq!(menu.entries()[0].chord, None);
        assert_eq!(menu.entries()[0].chord_label(), "-");
        assert_eq!(menu.problems().len(), 1);
        assert_eq!(
            menu.problems()[0].code.as_str(),
            "ui.plugin_shortcut_conflict"
        );
        // Ctrl+Z still means undo, and nothing else.
        assert_eq!(menu.command_for(Key::Z, Modifiers::COMMAND), None);
        assert_eq!(menu.log_problems(), 1);
    }

    #[test]
    fn the_second_plugin_to_ask_for_a_chord_does_not_get_it() {
        let mut registry = registry("a", &[desc("go", "Go", Some("Ctrl+Shift+G"))]);
        registry
            .register("b", &[desc("go", "Go too", Some("Ctrl+Shift+G"))])
            .unwrap();
        let menu = PluginMenu::register(&registry, &ShortcutMap::default_map());

        assert_eq!(
            menu.chord_for("a/go").map(super::chord_label),
            Some(SHIFT_G_LABEL.to_owned())
        );
        assert_eq!(menu.chord_for("b/go"), None);
        assert_eq!(menu.problems().len(), 1);
        assert_eq!(
            menu.command_for(Key::G, Modifiers::COMMAND | Modifiers::SHIFT),
            Some("a/go")
        );
    }

    #[test]
    fn a_chord_that_is_not_a_chord_is_reported_rather_than_guessed_at() {
        let registry = registry("p", &[desc("go", "Go", Some("Ctrl+Nope"))]);
        let menu = PluginMenu::register(&registry, &ShortcutMap::default_map());

        assert_eq!(menu.entries()[0].chord, None);
        assert_eq!(menu.problems().len(), 1);
        assert_eq!(menu.problems()[0].code.as_str(), "ui.plugin_invalid_chord");
    }

    #[test]
    fn an_empty_menu_claims_no_keys() {
        let menu = PluginMenu::empty();
        assert!(menu.is_empty());
        let mut events = vec![eframe::egui::Event::Key {
            key: Key::G,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }];
        assert!(menu.take_commands(&mut events).is_empty());
        assert_eq!(events.len(), 1, "the event is left for someone else");
    }
}
