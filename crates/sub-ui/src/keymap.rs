//! The user's `keymap.toml`: a small overlay on top of the shipped
//! [`DEFAULT_BINDINGS`](crate::shortcuts::DEFAULT_BINDINGS).
//!
//! Editors arriving from Premiere or Resolve bring their fingers with them,
//! so the shipped map is only a default (docs/PLAN.md §5.7). A file in the
//! config directory names actions by their stable
//! [`Action::id`](crate::shortcuts::Action::id) and gives each one a chord:
//!
//! ```toml
//! [bindings]
//! "editing.split_at_playhead" = "Ctrl+K"
//! "view.toggle_snapping" = "S"
//! "marking.add_marker" = "none"   # unbound
//! ```
//!
//! A keymap is configuration, never a reason to refuse to start: a missing
//! file leaves the defaults alone, and a file that is malformed or names an
//! action that does not exist loads as far as it can and reports the rest.
//! Every problem is a [`SubError`] with a stable code, collected in
//! [`LoadedKeymap::problems`] so the caller can log them and the shortcut help
//! window can show them.
//!
//! ```
//! use sub_ui::keymap::LoadedKeymap;
//! use sub_ui::shortcuts::Action;
//!
//! let loaded = LoadedKeymap::from_toml(
//!     "[bindings]\n\"editing.undo\" = \"Ctrl+U\"\n\"nope.nope\" = \"X\"\n",
//! );
//! assert_eq!(loaded.map.chord_label_for(Action::Undo), "Ctrl+U");
//! assert_eq!(loaded.problems.len(), 1, "the unknown action is reported");
//! ```

use std::path::{Path, PathBuf};

use eframe::egui::{Key, KeyboardShortcut, Modifiers};
use sub_core::SubError;

use crate::codes;
use crate::shortcuts::{Action, ShortcutMap};

/// The environment variable that overrides where the config directory is.
///
/// Set it and the editor reads `keymap.toml` from there instead of the
/// platform location; the tests use it, and so can a portable install.
pub const CONFIG_DIR_ENV: &str = "SUBORDINATE_CONFIG_DIR";

/// The name of the keymap file inside the config directory.
pub const KEYMAP_FILE_NAME: &str = "keymap.toml";

/// The table a keymap file puts its bindings in.
const BINDINGS_TABLE: &str = "bindings";

/// The chord value that means "this action has no chord".
const UNBOUND: &str = "none";

/// The editor's config directory, or `None` when the platform gives no home.
///
/// [`CONFIG_DIR_ENV`] wins when it is set to a non-empty value. Otherwise it
/// is `$XDG_CONFIG_HOME/subordinate` (falling back to `~/.config`) on Unix,
/// `%APPDATA%\subordinate` on Windows, and
/// `~/Library/Application Support/subordinate` on macOS.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(&non_empty_env)
}

/// [`config_dir`] against an arbitrary environment.
///
/// The lookup is a parameter so the tests can describe a Windows or a
/// home-less machine without mutating this process's environment, which is
/// unsafe and would race the other tests in the binary.
fn config_dir_from(lookup: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(dir) = lookup(CONFIG_DIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    platform_config_dir(lookup).map(|base| base.join("subordinate"))
}

/// The path `keymap.toml` is read from, if there is a config directory.
#[must_use]
pub fn keymap_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(KEYMAP_FILE_NAME))
}

/// The value of `name` in the environment, if it is set and not blank.
fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// The platform's per-user configuration root, before the app name is joined.
fn platform_config_dir(lookup: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        lookup("APPDATA").map(PathBuf::from)
    }
    #[cfg(target_os = "macos")]
    {
        lookup("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        lookup("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| lookup("HOME").map(|home| PathBuf::from(home).join(".config")))
    }
}

/// The action whose id is `id`, if any.
#[must_use]
pub fn action_by_id(id: &str) -> Option<Action> {
    Action::ALL.into_iter().find(|action| action.id() == id)
}

/// Splits a chord spec into its modifier words and its key word.
///
/// `+` is both the separator and a key name, so a spec ending in `+` binds
/// the plus key: a bare `+` is the key alone, and `Ctrl+` and `Ctrl++` both
/// read as Ctrl with the plus key.
fn split_chord(spec: &str) -> (&str, &str) {
    let spec = spec.trim();
    if let Some(head) = spec.strip_suffix('+') {
        return (head.strip_suffix('+').unwrap_or(head), "+");
    }
    spec.rsplit_once('+').unwrap_or(("", spec))
}

/// The modifier a single word names, if it names one.
///
/// `ctrl`, `cmd` and `command` all become [`Modifiers::COMMAND`], which is
/// Ctrl off macOS and Cmd on it, exactly as the shipped table writes them.
fn modifier_by_name(name: &str) -> Option<Modifiers> {
    match name.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "cmd" | "command" | "super" => Some(Modifiers::COMMAND),
        "shift" => Some(Modifiers::SHIFT),
        "alt" | "option" | "opt" => Some(Modifiers::ALT),
        _ => None,
    }
}

/// Parses a chord spec such as `Ctrl+Shift+Z` or `F1`.
///
/// Key names are egui's, so `Left`, `Space`, `Comma` and `,` all work.
///
/// # Errors
///
/// [`codes::KEYMAP_INVALID_CHORD`] when a word is neither a known modifier
/// nor, in last place, a known key.
pub fn parse_chord(spec: &str) -> Result<KeyboardShortcut, SubError> {
    let invalid = |reason: &str| {
        SubError::new(
            codes::KEYMAP_INVALID_CHORD,
            format!("{spec:?} is not a keyboard chord: {reason}"),
        )
        .with_detail("chord", spec)
    };
    let (modifier_words, key_word) = split_chord(spec);
    if key_word.is_empty() {
        return Err(invalid("it names no key"));
    }
    let mut modifiers = Modifiers::NONE;
    for word in modifier_words.split('+').filter(|word| !word.is_empty()) {
        let Some(modifier) = modifier_by_name(word) else {
            return Err(invalid(&format!("{word:?} is not a modifier")));
        };
        modifiers = modifiers.plus(modifier);
    }
    let Some(key) = Key::from_name(key_word.trim()) else {
        return Err(invalid(&format!("{key_word:?} is not a key")));
    };
    Ok(KeyboardShortcut::new(modifiers, key))
}

/// A chord written the way [`parse_chord`] reads it, e.g. `Ctrl+Shift+Z`.
///
/// This is not the help window's label: it never uses the macOS glyphs, so a
/// keymap file stays readable and portable between machines.
#[must_use]
pub fn chord_spec(chord: KeyboardShortcut) -> String {
    let mut spec = String::new();
    let modifiers = chord.modifiers;
    if modifiers.ctrl || modifiers.command || modifiers.mac_cmd {
        spec.push_str("Ctrl+");
    }
    if modifiers.alt {
        spec.push_str("Alt+");
    }
    if modifiers.shift {
        spec.push_str("Shift+");
    }
    spec.push_str(chord.logical_key.name());
    spec
}

/// A keymap after loading: the map in force, and everything wrong with the
/// file that produced it.
///
/// The map is always usable. `problems` is empty when the file was absent or
/// wholly valid; otherwise it holds one [`SubError`] per rejected entry, and
/// the entries that were fine are still applied.
#[derive(Debug, Clone)]
pub struct LoadedKeymap {
    /// The bindings the editor should run with.
    pub map: ShortcutMap,
    /// The file this came from, when one was read.
    pub source: Option<PathBuf>,
    /// One error per rejected entry, in file order.
    pub problems: Vec<SubError>,
}

impl Default for LoadedKeymap {
    fn default() -> Self {
        Self::defaults()
    }
}

impl LoadedKeymap {
    /// The shipped map, with no file behind it.
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            map: ShortcutMap::default_map(),
            source: None,
            problems: Vec::new(),
        }
    }

    /// Whether anything in the file was rejected.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }

    /// The shipped map with the overrides in `text` applied.
    ///
    /// Malformed TOML is one problem and leaves the defaults untouched; a bad
    /// entry inside otherwise valid TOML is one problem and leaves the other
    /// entries applied.
    #[must_use]
    pub fn from_toml(text: &str) -> Self {
        let mut loaded = Self::defaults();
        let table: toml::Table = match toml::from_str(text) {
            Ok(table) => table,
            Err(error) => {
                loaded.problems.push(
                    SubError::new(
                        codes::KEYMAP_PARSE,
                        format!("keymap.toml is not valid TOML: {error}"),
                    )
                    .with_detail("parse_error", error.to_string()),
                );
                return loaded;
            }
        };
        let Some(bindings) = table.get(BINDINGS_TABLE) else {
            // No [bindings] table is a keymap that overrides nothing, which is
            // what an empty or commented-out file means.
            return loaded;
        };
        let Some(bindings) = bindings.as_table() else {
            loaded.problems.push(
                SubError::new(
                    codes::KEYMAP_PARSE,
                    format!("[{BINDINGS_TABLE}] must be a table of action id to chord"),
                )
                .with_detail("found", bindings.type_str()),
            );
            return loaded;
        };
        for (id, value) in bindings {
            match loaded.apply_entry(id, value) {
                Ok(()) => {}
                Err(problem) => loaded.problems.push(problem),
            }
        }
        loaded
    }

    /// Applies one `id = chord` entry to the map.
    fn apply_entry(&mut self, id: &str, value: &toml::Value) -> Result<(), SubError> {
        let Some(action) = action_by_id(id) else {
            return Err(SubError::new(
                codes::KEYMAP_UNKNOWN_ACTION,
                format!("{id:?} is not an action this editor has"),
            )
            .with_detail("action", id));
        };
        let Some(spec) = value.as_str() else {
            return Err(SubError::new(
                codes::KEYMAP_PARSE,
                format!("the chord for {id:?} must be a string"),
            )
            .with_detail("action", id)
            .with_detail("found", value.type_str()));
        };
        if spec.trim().eq_ignore_ascii_case(UNBOUND) || spec.trim().is_empty() {
            self.map.unbind(action);
            return Ok(());
        }
        let chord = parse_chord(spec).map_err(|error| error.with_detail("action", id))?;
        self.map.rebind(action, chord);
        Ok(())
    }

    /// The shipped map with `path`'s overrides applied, if the file exists.
    ///
    /// A missing file is not a problem: it is the ordinary case of a user who
    /// never wrote one. A file that cannot be read is one problem, and the
    /// defaults are used.
    #[must_use]
    pub fn from_path(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self::defaults();
            }
            Err(error) => {
                let mut loaded = Self::defaults();
                loaded.source = Some(path.to_path_buf());
                loaded.problems.push(
                    SubError::new(
                        codes::KEYMAP_UNREADABLE,
                        format!("{} could not be read: {error}", path.display()),
                    )
                    .with_detail("path", path.display().to_string()),
                );
                return loaded;
            }
        };
        let mut loaded = Self::from_toml(&text);
        loaded.source = Some(path.to_path_buf());
        loaded
    }

    /// The keymap from the config directory, or the defaults when there is
    /// none.
    #[must_use]
    pub fn load() -> Self {
        keymap_path().map_or_else(Self::defaults, |path| Self::from_path(&path))
    }

    /// Logs where the map came from and every problem the file had, and
    /// returns how many problems there were.
    ///
    /// Called once at startup, for the same reason conflicts are: a shortcut
    /// that silently does nothing is worse than a log line.
    pub fn log_problems(&self) -> usize {
        match &self.source {
            Some(path) if self.problems.is_empty() => {
                log::info!("keyboard map loaded from {}", path.display());
            }
            Some(path) => {
                log::warn!(
                    "{} problem(s) in {}; the rest of the map was applied",
                    self.problems.len(),
                    path.display()
                );
            }
            None => log::debug!("no keymap.toml; using the shipped keyboard map"),
        }
        for problem in &self.problems {
            log::warn!("keymap: [{}] {}", problem.code, problem.message);
        }
        self.problems.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CONFIG_DIR_ENV, KEYMAP_FILE_NAME, LoadedKeymap, action_by_id, chord_spec, config_dir,
        config_dir_from, keymap_path, parse_chord,
    };
    use crate::codes;
    use crate::shortcuts::{Action, ShortcutMap, ShortcutsWindow};
    use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};
    /// The example map shipped beside the crate, checked at compile time to
    /// exist and at run time to load.
    const PREMIERE_EXAMPLE: &str = include_str!("../keymaps/premiere.toml");

    /// An environment built from a list of pairs, for [`config_dir_from`].
    ///
    /// Blank values read as unset, exactly as the real lookup treats them.
    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .filter(|value| !value.trim().is_empty())
        }
    }

    /// A scratch directory unique to `name`, emptied first.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sub-ui-keymap-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the scratch directory should be creatable");
        dir
    }

    #[test]
    fn chords_round_trip_through_their_spec() {
        for spec in [
            "L",
            "F1",
            "Ctrl+Z",
            "Ctrl+Shift+Z",
            "Alt+Left",
            "Space",
            "Comma",
            "Period",
            "Home",
        ] {
            let chord = parse_chord(spec).unwrap_or_else(|error| panic!("{spec}: {error}"));
            let round_tripped = chord_spec(chord);
            assert_eq!(
                parse_chord(&round_tripped).expect("a formatted spec parses"),
                chord,
                "{spec} formatted as {round_tripped}"
            );
        }
    }

    #[test]
    fn every_shipped_chord_has_a_spec_that_parses_back() {
        for binding in ShortcutMap::default_map().bindings() {
            let spec = chord_spec(binding.chord);
            assert_eq!(
                parse_chord(&spec).expect("a shipped chord formats to a valid spec"),
                binding.chord,
                "{} formatted as {spec}",
                binding.action.id()
            );
        }
    }

    #[test]
    fn modifier_names_are_case_insensitive_and_platform_neutral() {
        let expected = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
        for spec in [
            "Ctrl+Shift+Z",
            "ctrl+shift+z",
            "CMD+SHIFT+Z",
            "Command+Shift+Z",
        ] {
            assert_eq!(
                parse_chord(spec).expect("a valid chord"),
                expected,
                "{spec}"
            );
        }
    }

    #[test]
    fn the_plus_key_can_be_bound() {
        let plus = parse_chord("+").expect("a bare plus is the plus key");
        assert_eq!(plus.logical_key, Key::Plus);
        assert_eq!(plus.modifiers, Modifiers::NONE);
        for spec in ["Ctrl++", "Ctrl+"] {
            let with_ctrl = parse_chord(spec).expect("ctrl and the plus key");
            assert_eq!(with_ctrl.logical_key, Key::Plus, "{spec}");
            assert!(
                with_ctrl.modifiers.matches_exact(Modifiers::COMMAND),
                "{spec}"
            );
        }
    }

    #[test]
    fn a_bad_chord_is_an_error_with_a_stable_code() {
        for spec in ["", "   ", "Hyper+Z", "Ctrl+Nope", "Ctrl+Shift"] {
            let Err(error) = parse_chord(spec) else {
                panic!("{spec:?} should not parse");
            };
            assert_eq!(error.code, codes::KEYMAP_INVALID_CHORD, "{spec:?}");
        }
    }

    #[test]
    fn action_ids_resolve_and_unknown_ones_do_not() {
        for action in Action::ALL {
            assert_eq!(action_by_id(action.id()), Some(action));
        }
        assert_eq!(action_by_id("editing.teleport"), None);
    }

    #[test]
    fn a_file_overrides_only_the_actions_it_names() {
        let loaded = LoadedKeymap::from_toml(
            "[bindings]\n\"editing.split_at_playhead\" = \"Ctrl+Shift+K\"\n",
        );
        assert!(loaded.is_clean(), "problems: {:?}", loaded.problems);
        assert_eq!(
            loaded
                .map
                .action_for(Key::K, Modifiers::COMMAND.plus(Modifiers::SHIFT)),
            Some(Action::SplitAtPlayhead)
        );
        assert_eq!(loaded.map.action_for(Key::K, Modifiers::COMMAND), None);
        // Everything else is untouched.
        assert_eq!(
            loaded.map.action_for(Key::L, Modifiers::NONE),
            Some(Action::PlayForward)
        );
        assert!(loaded.map.conflicts().is_empty());
    }

    #[test]
    fn none_unbinds_an_action() {
        let loaded = LoadedKeymap::from_toml("[bindings]\n\"view.toggle_snapping\" = \"none\"\n");
        assert!(loaded.is_clean(), "problems: {:?}", loaded.problems);
        assert_eq!(loaded.map.action_for(Key::S, Modifiers::NONE), None);
        assert_eq!(loaded.map.chord_for(Action::ToggleSnapping), None);
        assert_eq!(loaded.map.chord_label_for(Action::ToggleSnapping), "-");
    }

    #[test]
    fn an_empty_file_and_one_without_bindings_change_nothing() {
        for text in ["", "# just a comment\n", "[other]\nkey = 1\n"] {
            let loaded = LoadedKeymap::from_toml(text);
            assert!(loaded.is_clean(), "{text:?}: {:?}", loaded.problems);
            assert_eq!(loaded.map, ShortcutMap::default_map(), "{text:?}");
        }
    }

    #[test]
    fn bad_entries_are_reported_and_the_good_ones_still_apply() {
        let loaded = LoadedKeymap::from_toml(
            "[bindings]\n\
             \"editing.undo\" = \"Ctrl+U\"\n\
             \"editing.teleport\" = \"Ctrl+T\"\n\
             \"editing.redo\" = \"Ctrl+Nope\"\n\
             \"marking.add_marker\" = 7\n",
        );
        assert_eq!(loaded.problems.len(), 3, "problems: {:?}", loaded.problems);
        let seen: Vec<&str> = loaded
            .problems
            .iter()
            .map(|problem| problem.code.as_str())
            .collect();
        assert!(
            seen.contains(&codes::KEYMAP_UNKNOWN_ACTION.as_str()),
            "{seen:?}"
        );
        assert!(
            seen.contains(&codes::KEYMAP_INVALID_CHORD.as_str()),
            "{seen:?}"
        );
        assert!(seen.contains(&codes::KEYMAP_PARSE.as_str()), "{seen:?}");
        // The valid entry survived; the rejected ones kept their defaults.
        assert_eq!(
            loaded.map.action_for(Key::U, Modifiers::COMMAND),
            Some(Action::Undo)
        );
        assert_eq!(
            loaded
                .map
                .action_for(Key::Z, Modifiers::COMMAND.plus(Modifiers::SHIFT)),
            Some(Action::Redo)
        );
        assert_eq!(
            loaded.map.action_for(Key::M, Modifiers::NONE),
            Some(Action::AddMarker)
        );
        assert_eq!(loaded.log_problems(), 3);
    }

    #[test]
    fn malformed_toml_is_reported_and_leaves_the_defaults() {
        let loaded = LoadedKeymap::from_toml("[bindings\n\"editing.undo\" = \n");
        assert_eq!(loaded.problems.len(), 1);
        assert_eq!(loaded.problems[0].code, codes::KEYMAP_PARSE);
        assert_eq!(loaded.map, ShortcutMap::default_map());
    }

    #[test]
    fn a_bindings_key_that_is_not_a_table_is_reported() {
        let loaded = LoadedKeymap::from_toml("bindings = \"oops\"\n");
        assert_eq!(loaded.problems.len(), 1);
        assert_eq!(loaded.problems[0].code, codes::KEYMAP_PARSE);
        assert_eq!(loaded.map, ShortcutMap::default_map());
    }

    #[test]
    fn a_missing_file_is_not_a_problem() {
        let dir = scratch_dir("missing");
        let loaded = LoadedKeymap::from_path(&dir.join(KEYMAP_FILE_NAME));
        assert!(loaded.is_clean(), "problems: {:?}", loaded.problems);
        assert_eq!(loaded.source, None);
        assert_eq!(loaded.map, ShortcutMap::default_map());
        assert_eq!(loaded.log_problems(), 0);
    }

    #[test]
    fn an_unreadable_path_is_reported_not_fatal() {
        // A directory where a file is expected: readable as a path, not as a
        // file, which is the shape of every "your config is broken" case.
        let dir = scratch_dir("unreadable");
        let path = dir.join(KEYMAP_FILE_NAME);
        std::fs::create_dir(&path).expect("the stand-in directory should be creatable");
        let loaded = LoadedKeymap::from_path(&path);
        assert_eq!(loaded.problems.len(), 1, "problems: {:?}", loaded.problems);
        assert_eq!(loaded.problems[0].code, codes::KEYMAP_UNREADABLE);
        assert_eq!(loaded.map, ShortcutMap::default_map());
    }

    #[test]
    fn a_file_in_the_config_dir_is_what_the_editor_loads() {
        let dir = scratch_dir("load");
        std::fs::write(
            dir.join(KEYMAP_FILE_NAME),
            "[bindings]\n\"transport.play_forward\" = \"Ctrl+Alt+L\"\n",
        )
        .expect("the keymap should be writable");
        // `load` is `from_path` on `keymap_path`, and the path is what the
        // environment decides; both halves are checked, without touching this
        // process's environment.
        let path = config_dir_from(&env_of(&[(CONFIG_DIR_ENV, &dir.to_string_lossy())]))
            .expect("the override names a config directory")
            .join(KEYMAP_FILE_NAME);
        assert_eq!(path, dir.join(KEYMAP_FILE_NAME));
        let loaded = LoadedKeymap::from_path(&path);
        assert!(loaded.is_clean(), "problems: {:?}", loaded.problems);
        assert_eq!(loaded.source, Some(path));
        assert_eq!(
            loaded
                .map
                .action_for(Key::L, Modifiers::COMMAND.plus(Modifiers::ALT)),
            Some(Action::PlayForward)
        );
        assert_eq!(loaded.log_problems(), 0);
    }

    #[test]
    fn the_config_dir_follows_the_platform_and_its_override() {
        // The override wins wherever it is set, and is used verbatim.
        let overridden = config_dir_from(&env_of(&[
            (CONFIG_DIR_ENV, "/somewhere/else"),
            ("HOME", "/home/editor"),
            ("XDG_CONFIG_HOME", "/home/editor/.config"),
            ("APPDATA", "C:\\Users\\editor\\AppData\\Roaming"),
        ]));
        assert_eq!(
            overridden,
            Some(std::path::PathBuf::from("/somewhere/else"))
        );

        // A blank override is no override, and with nothing else in the
        // environment there is no config file at all: the editor keeps its
        // shipped map rather than failing.
        assert_eq!(config_dir_from(&env_of(&[(CONFIG_DIR_ENV, "  ")])), None);
        assert_eq!(config_dir_from(&env_of(&[])), None);

        // The platform root has the app name joined onto it.
        let platform = config_dir_from(&env_of(&[
            ("HOME", "/home/editor"),
            ("XDG_CONFIG_HOME", "/home/editor/.config"),
            ("APPDATA", "C:\\Users\\editor\\AppData\\Roaming"),
        ]))
        .expect("a machine with a home has a config directory");
        assert!(
            platform.ends_with("subordinate"),
            "the app name should be joined on: {}",
            platform.display()
        );

        // The real accessors agree with each other on this machine.
        assert_eq!(
            keymap_path(),
            config_dir().map(|dir| dir.join(KEYMAP_FILE_NAME))
        );
    }

    #[test]
    fn the_premiere_example_loads_cleanly_and_changes_the_map() {
        let loaded = LoadedKeymap::from_toml(PREMIERE_EXAMPLE);
        assert!(
            loaded.is_clean(),
            "the shipped example should load cleanly: {:?}",
            loaded.problems
        );
        assert!(
            loaded.map.conflicts().is_empty(),
            "the shipped example should not double-bind a chord: {:?}",
            loaded.map.conflicts()
        );
        loaded
            .map
            .validate()
            .expect("the shipped example should validate");
        assert_ne!(
            loaded.map,
            ShortcutMap::default_map(),
            "the example should actually remap something"
        );
        // The Premiere keys editors come for.
        assert_eq!(
            loaded.map.action_for(Key::C, Modifiers::NONE),
            Some(Action::SplitAtPlayhead),
            "C is the razor in Premiere"
        );
        assert_eq!(
            loaded.map.action_for(Key::M, Modifiers::COMMAND),
            Some(Action::AddMarker),
            "Ctrl+M is the marker in Premiere"
        );
        // Every action is still reachable, so the help window is complete.
        for action in Action::ALL {
            assert!(
                loaded.map.chord_for(action).is_some(),
                "{} lost its chord",
                action.id()
            );
        }
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

    /// Paints the help list for `loaded` and returns every string drawn.
    fn painted_help(loaded: &LoadedKeymap) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        for _ in 0..2 {
            texts.clear();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ShortcutsWindow::ui_with_problems(ui, &loaded.map, &loaded.problems);
            });
            for clipped in &output.shapes {
                collect_text(&clipped.shape, &mut texts);
            }
            output.textures_delta.clear();
        }
        texts
    }

    #[test]
    fn the_help_window_shows_the_remapped_chord_not_the_default() {
        let loaded = LoadedKeymap::from_toml(PREMIERE_EXAMPLE);
        let painted = painted_help(&loaded);
        for binding in loaded.map.bindings() {
            let chord = binding.chord_label();
            assert!(
                painted.iter().any(|text| text == chord.as_str()),
                "the active chord {chord} for {} is missing: {painted:?}",
                binding.action.id()
            );
        }
        let default_razor = ShortcutMap::default_map().chord_label_for(Action::SplitAtPlayhead);
        assert!(
            !painted.iter().any(|text| text == default_razor.as_str()),
            "the help window still shows the default {default_razor}: {painted:?}"
        );
    }

    #[test]
    fn the_help_window_reports_keymap_problems() {
        let loaded = LoadedKeymap::from_toml("[bindings]\n\"editing.teleport\" = \"Ctrl+T\"\n");
        let painted = painted_help(&loaded);
        assert!(
            painted
                .iter()
                .any(|text| text.contains(codes::KEYMAP_UNKNOWN_ACTION.as_str())),
            "the problem is not shown: {painted:?}"
        );
    }
}
