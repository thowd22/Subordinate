//! Fullscreen review on a chosen display: which monitor the pop-out viewer
//! takes over, and how that choice survives a restart (docs/PLAN.md §5.7).
//!
//! The pop-out viewer of [`crate::popout`] is already the picture on black in
//! a window of its own. Fullscreen is that window with the desktop taken out
//! of the way, on whichever display the user picked: a client watching a cut
//! on the TV sees the frame and nothing else.
//!
//! Three pieces meet here.
//!
//! - [`Monitor`] and [`MonitorList`] are the displays the picker offers. The
//!   order is winit's `available_monitors()` order, because that is what
//!   [`egui::ViewportBuilder::with_monitor`] indexes into. eframe 0.36 hands
//!   an application no way to enumerate displays, so the list the editor can
//!   describe by itself is the one it is painting on
//!   ([`MonitorList::from_context`]); [`FullscreenState::choose_index`] is the
//!   escape hatch that names a display the editor cannot see by number.
//!   Whether a real second head lights up is TASK-118's job on hardware.
//! - [`FullscreenSettings`] is the choice, and it is written to
//!   `fullscreen.json` beside `layout.json` and `keymap.toml` in the per-user
//!   config directory ([`config_dir`](crate::keymap::config_dir)). Like the
//!   layout, it is configuration and never a reason to refuse to start: a
//!   missing file is "no choice yet", and a malformed one is that plus a
//!   [`SubError`] for the caller to log.
//! - [`monitor_picker_ui`] is the picker itself. It mutates nothing but the
//!   choice and hands back a [`FullscreenAction`] for the application to apply
//!   to the pop-out window.
//!
//! ```
//! use sub_ui::fullscreen::{FullscreenSettings, FullscreenState, Monitor, MonitorList};
//!
//! let mut state = FullscreenState::new();
//! state.set_monitors(MonitorList::new(vec![
//!     Monitor::new("eDP-1", [1920.0, 1080.0], true),
//!     Monitor::new("HDMI-1", [3840.0, 2160.0], false),
//! ]));
//! assert_eq!(state.monitor_index(), None, "the window manager decides at first");
//!
//! // Picking the TV is remembered by name as well as by index, so a display
//! // that moved in the list is still found after a restart.
//! state.choose(Some(1));
//! assert_eq!(state.monitor_index(), Some(1));
//! let json = state.to_json().expect("the choice serialises");
//! let restored = FullscreenSettings::from_json(&json).expect("and reads back");
//! assert_eq!(restored.monitor_name(), Some("HDMI-1"));
//! ```

use std::path::{Path, PathBuf};

use eframe::egui;
use serde::{Deserialize, Serialize};
use sub_core::SubError;

use crate::codes;
use crate::keymap::config_dir;

/// The name of the fullscreen settings file inside the config directory.
pub const FULLSCREEN_FILE_NAME: &str = "fullscreen.json";

/// The schema version written into that file.
///
/// A file from another version is dropped rather than migrated: the choice is
/// one line of configuration, and re-picking a display costs a click.
pub const FULLSCREEN_VERSION: u32 = 1;

/// The picker's heading.
pub const PICKER_TITLE: &str = "Fullscreen display";

/// The picker's row for "wherever the window already is".
pub const CURRENT_DISPLAY_LABEL: &str = "Current display";

/// The button that takes the pop-out fullscreen.
pub const ENTER_LABEL: &str = "Go fullscreen";

/// The button that brings it back to a window.
pub const LEAVE_LABEL: &str = "Leave fullscreen";

/// The button beside the display-number box.
pub const USE_NUMBER_LABEL: &str = "Use display number";

/// The highest display number the picker's box offers.
///
/// Sixteen heads is past any review setup; the box is an escape hatch, not a
/// configuration surface.
pub const MAX_DISPLAYS: usize = 16;

/// One display the pop-out can take over.
///
/// `index` is the display's position in winit's `available_monitors()` order,
/// which is the index [`egui::ViewportBuilder::with_monitor`] and
/// [`egui::ViewportCommand::SetMonitor`] take. `size` is in points, as the
/// window system reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    /// The display's position in the platform's monitor list.
    pub index: usize,
    /// What the platform calls it, or a made-up name when it has none.
    pub name: String,
    /// Its size in points.
    pub size: [f32; 2],
    /// Whether the platform calls it the primary display.
    pub primary: bool,
}

impl Monitor {
    /// A display at index 0; [`MonitorList::new`] renumbers it.
    #[must_use]
    pub fn new(name: impl Into<String>, size: [f32; 2], primary: bool) -> Self {
        Self {
            index: 0,
            name: name.into(),
            size,
            primary,
        }
    }

    /// The row the picker shows: the number, the name, the size.
    #[must_use]
    pub fn label(&self) -> String {
        let primary = if self.primary { " (primary)" } else { "" };
        format!(
            "{}: {} - {:.0}x{:.0}{primary}",
            self.index + 1,
            self.name,
            self.size[0],
            self.size[1]
        )
    }
}

/// The displays the picker offers, in the platform's own order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MonitorList {
    /// The displays, renumbered so `index` is the position in this list.
    monitors: Vec<Monitor>,
}

impl MonitorList {
    /// A list of displays, numbered by their position in `monitors`.
    #[must_use]
    pub fn new(monitors: Vec<Monitor>) -> Self {
        let monitors = monitors
            .into_iter()
            .enumerate()
            .map(|(index, monitor)| Monitor { index, ..monitor })
            .collect();
        Self { monitors }
    }

    /// The one display an eframe application can describe by itself: the one
    /// the window it is painting is on.
    ///
    /// egui reports the current viewport's monitor size and nothing else, so
    /// this is a list of one. It is still worth having: the picker always has
    /// a display to name, and the size in it is the real one.
    #[must_use]
    pub fn from_context(ctx: &egui::Context) -> Self {
        match ctx.input(|input| input.viewport().monitor_size) {
            Some(size) => Self::new(vec![Monitor::new(
                CURRENT_DISPLAY_LABEL,
                [size.x, size.y],
                true,
            )]),
            None => Self::default(),
        }
    }

    /// The displays, in order.
    #[must_use]
    pub fn as_slice(&self) -> &[Monitor] {
        &self.monitors
    }

    /// How many displays are listed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.monitors.len()
    }

    /// Whether no display could be listed at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.monitors.is_empty()
    }

    /// The display at `index`, if the list reaches that far.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Monitor> {
        self.monitors.get(index)
    }

    /// The display a saved choice names.
    ///
    /// The name wins over the index, because a display that moved in the
    /// platform's list is still the display the user picked. The index is the
    /// fallback, for a monitor the platform renamed between runs.
    #[must_use]
    pub fn resolve(&self, choice: &MonitorChoice) -> Option<&Monitor> {
        self.monitors
            .iter()
            .find(|monitor| monitor.name == choice.name)
            .or_else(|| self.get(choice.index))
    }
}

/// A display as a settings file names it.
///
/// Both halves are written: the index is what the window system takes, and
/// the name is what survives the displays being re-ordered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorChoice {
    /// The display's position in the platform's list when it was chosen.
    pub index: usize,
    /// What the platform called it then.
    pub name: String,
}

/// What the user chose, as it is written to `fullscreen.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FullscreenSettings {
    /// The display fullscreen takes over, or `None` for the one the pop-out
    /// window is already on.
    pub monitor: Option<MonitorChoice>,
}

/// The file's shape, version and all.
#[derive(Debug, Serialize, Deserialize)]
struct StoredSettings {
    /// The schema version; see [`FULLSCREEN_VERSION`].
    version: u32,
    /// The chosen display, when one was chosen.
    #[serde(default)]
    monitor: Option<MonitorChoice>,
}

impl FullscreenSettings {
    /// The index the window system is given, when a display was chosen.
    #[must_use]
    pub fn monitor_index(&self) -> Option<usize> {
        self.monitor.as_ref().map(|choice| choice.index)
    }

    /// The name of the chosen display, when one was chosen.
    #[must_use]
    pub fn monitor_name(&self) -> Option<&str> {
        self.monitor.as_ref().map(|choice| choice.name.as_str())
    }

    /// Reads the settings from the text of a `fullscreen.json`.
    ///
    /// # Errors
    ///
    /// [`codes::FULLSCREEN_PARSE`] when the text is not JSON of this schema
    /// version.
    pub fn from_json(text: &str) -> Result<Self, SubError> {
        let stored: StoredSettings = serde_json::from_str(text).map_err(|error| {
            SubError::wrap(
                codes::FULLSCREEN_PARSE,
                "the fullscreen settings file is malformed",
                &error,
            )
        })?;
        if stored.version != FULLSCREEN_VERSION {
            return Err(SubError::new(
                codes::FULLSCREEN_PARSE,
                format!(
                    "the fullscreen settings file is version {}, and this build writes version \
                     {FULLSCREEN_VERSION}",
                    stored.version
                ),
            )
            .with_detail("version", stored.version));
        }
        Ok(Self {
            monitor: stored.monitor,
        })
    }

    /// The settings as the text of a `fullscreen.json`.
    ///
    /// # Errors
    ///
    /// [`codes::FULLSCREEN_UNWRITABLE`] if the choice cannot be serialised,
    /// which would be a bug in this crate rather than anything the user did.
    pub fn to_json(&self) -> Result<String, SubError> {
        let stored = StoredSettings {
            version: FULLSCREEN_VERSION,
            monitor: self.monitor.clone(),
        };
        serde_json::to_string_pretty(&stored).map_err(|error| {
            SubError::wrap(
                codes::FULLSCREEN_UNWRITABLE,
                "the fullscreen settings could not be serialised",
                &error,
            )
        })
    }
}

/// What the picker asked the application for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenAction {
    /// Use this display from now on. The state has already recorded it.
    Choose(Option<usize>),
    /// Take the pop-out fullscreen.
    Enter,
    /// Put it back in a window.
    Leave,
}

/// The fullscreen choice at runtime: the displays on offer, what was picked,
/// and what is on disk.
#[derive(Debug, Clone)]
pub struct FullscreenState {
    /// The displays the picker lists.
    monitors: MonitorList,
    /// The choice in force.
    settings: FullscreenSettings,
    /// The JSON last read from or written to disk, so
    /// [`FullscreenState::persist_to_dir`] can tell an unchanged choice from a
    /// changed one without reading the file back.
    persisted: Option<String>,
    /// The number in the picker's display-number box, 1-based as a user counts
    /// displays. Not part of the choice, so it is never written to disk.
    display_number: usize,
}

impl Default for FullscreenState {
    fn default() -> Self {
        Self {
            monitors: MonitorList::default(),
            settings: FullscreenSettings::default(),
            persisted: None,
            display_number: 1,
        }
    }
}

impl FullscreenState {
    /// No displays listed and no choice made.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the displays the picker lists.
    ///
    /// A choice that named a display by name is re-pointed at wherever that
    /// display now sits, so unplugging a monitor and plugging it back in does
    /// not silently move the review picture to somebody's laptop screen.
    pub fn set_monitors(&mut self, monitors: MonitorList) {
        if let Some(choice) = &self.settings.monitor
            && let Some(monitor) = monitors.resolve(choice)
        {
            self.settings.monitor = Some(MonitorChoice {
                index: monitor.index,
                name: monitor.name.clone(),
            });
        }
        self.monitors = monitors;
    }

    /// Lists the display the editor's own window is on, if nothing is listed.
    ///
    /// Called once a frame by the application: it costs an input read, and it
    /// means the picker always has a display to offer even on the first frame.
    pub fn refresh_from_context(&mut self, ctx: &egui::Context) {
        if self.monitors.is_empty() {
            let monitors = MonitorList::from_context(ctx);
            if !monitors.is_empty() {
                self.set_monitors(monitors);
            }
        }
    }

    /// The displays the picker lists.
    #[must_use]
    pub const fn monitors(&self) -> &MonitorList {
        &self.monitors
    }

    /// The choice as it is written to disk.
    #[must_use]
    pub const fn settings(&self) -> &FullscreenSettings {
        &self.settings
    }

    /// The index the pop-out window is given, when a display was chosen.
    #[must_use]
    pub fn monitor_index(&self) -> Option<usize> {
        self.settings.monitor_index()
    }

    /// The chosen display, when it is one the list knows about.
    #[must_use]
    pub fn chosen(&self) -> Option<&Monitor> {
        self.settings
            .monitor
            .as_ref()
            .and_then(|choice| self.monitors.resolve(choice))
    }

    /// Chooses the display at `index` in the list, or the current one for
    /// `None`.
    ///
    /// An index past the end of the list is ignored here; naming a display the
    /// editor cannot see is [`FullscreenState::choose_index`]'s job.
    pub fn choose(&mut self, index: Option<usize>) {
        match index {
            None => {
                self.settings.monitor = None;
                self.display_number = 1;
            }
            Some(index) => {
                if let Some(monitor) = self.monitors.get(index) {
                    self.settings.monitor = Some(MonitorChoice {
                        index,
                        name: monitor.name.clone(),
                    });
                    self.display_number = index + 1;
                }
            }
        }
    }

    /// Chooses a display by number, whether or not the editor can see it.
    ///
    /// eframe gives an application no display enumeration, so a second head is
    /// often a number the user knows and the editor does not. The number is
    /// stored as it stands and handed to the window system, which leaves the
    /// window where it is when it has no display at that index.
    pub fn choose_index(&mut self, index: usize) {
        let name = self.monitors.get(index).map_or_else(
            || format!("Display {}", index + 1),
            |monitor| monitor.name.clone(),
        );
        self.settings.monitor = Some(MonitorChoice { index, name });
        self.display_number = index + 1;
    }

    /// The number in the picker's display-number box, 1-based.
    #[must_use]
    pub const fn display_number(&self) -> usize {
        self.display_number
    }

    /// Whether the choice differs from what was last read or written.
    ///
    /// A choice that has never been loaded or saved counts as changed, so a
    /// first run writes the file out once.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        match (&self.persisted, self.to_json()) {
            (Some(saved), Ok(current)) => saved != &current,
            _ => true,
        }
    }

    /// The choice as the text of a `fullscreen.json`.
    ///
    /// # Errors
    ///
    /// [`codes::FULLSCREEN_UNWRITABLE`], as [`FullscreenSettings::to_json`].
    pub fn to_json(&self) -> Result<String, SubError> {
        self.settings.to_json()
    }

    /// Writes the choice to the per-user config directory, if it changed.
    ///
    /// Returns whether a file was written. There being no config directory at
    /// all is not an error: nothing is saved and `false` comes back.
    ///
    /// # Errors
    ///
    /// [`codes::FULLSCREEN_UNWRITABLE`] when the directory cannot be created
    /// or the file cannot be written.
    pub fn persist(&mut self) -> Result<bool, SubError> {
        let Some(dir) = config_dir() else {
            return Ok(false);
        };
        self.persist_to_dir(&dir)
    }

    /// [`FullscreenState::persist`] against an arbitrary config directory.
    ///
    /// # Errors
    ///
    /// [`codes::FULLSCREEN_UNWRITABLE`] when the directory cannot be created
    /// or the file cannot be written.
    pub fn persist_to_dir(&mut self, dir: &Path) -> Result<bool, SubError> {
        let text = self.to_json()?;
        if self.persisted.as_ref() == Some(&text) {
            return Ok(false);
        }
        std::fs::create_dir_all(dir).map_err(|error| {
            SubError::wrap(
                codes::FULLSCREEN_UNWRITABLE,
                format!("{} could not be created", dir.display()),
                &error,
            )
            .with_detail("path", dir.display().to_string())
        })?;
        let path = dir.join(FULLSCREEN_FILE_NAME);
        std::fs::write(&path, &text).map_err(|error| {
            SubError::wrap(
                codes::FULLSCREEN_UNWRITABLE,
                format!("{} could not be written", path.display()),
                &error,
            )
            .with_detail("path", path.display().to_string())
        })?;
        self.persisted = Some(text);
        Ok(true)
    }

    /// Reads the choice from the per-user config directory.
    #[must_use]
    pub fn load() -> LoadedFullscreen {
        config_dir().map_or_else(LoadedFullscreen::default, |dir| Self::load_from_dir(&dir))
    }

    /// [`FullscreenState::load`] against an arbitrary config directory.
    #[must_use]
    pub fn load_from_dir(dir: &Path) -> LoadedFullscreen {
        let path = dir.join(FULLSCREEN_FILE_NAME);
        if !path.exists() {
            return LoadedFullscreen::default();
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                let problem = SubError::wrap(
                    codes::FULLSCREEN_UNREADABLE,
                    format!("{} could not be read", path.display()),
                    &error,
                )
                .with_detail("path", path.display().to_string());
                return LoadedFullscreen {
                    state: Self::new(),
                    source: Some(path),
                    problems: vec![problem],
                };
            }
        };
        match FullscreenSettings::from_json(&text) {
            Ok(settings) => {
                let display_number = settings.monitor_index().map_or(1, |index| index + 1);
                LoadedFullscreen {
                    state: Self {
                        monitors: MonitorList::default(),
                        settings,
                        persisted: Some(text),
                        display_number,
                    },
                    source: Some(path),
                    problems: Vec::new(),
                }
            }
            Err(problem) => {
                let problem = problem.with_detail("path", path.display().to_string());
                LoadedFullscreen {
                    state: Self::new(),
                    source: Some(path),
                    problems: vec![problem],
                }
            }
        }
    }
}

/// What [`FullscreenState::load`] found.
#[derive(Debug, Default)]
pub struct LoadedFullscreen {
    /// The choice in force: the file's, or none when it could not be used.
    pub state: FullscreenState,
    /// The file it was read from, when there was one.
    pub source: Option<PathBuf>,
    /// Everything wrong with the file. Empty when there was no file.
    pub problems: Vec<SubError>,
}

impl LoadedFullscreen {
    /// Logs where the choice came from and what the file got wrong, and
    /// returns how many problems there were.
    pub fn log_problems(&self) -> usize {
        match &self.source {
            Some(path) if self.problems.is_empty() => {
                log::info!("fullscreen settings loaded from {}", path.display());
            }
            Some(path) => log::warn!(
                "{} problem(s) in {}; no display is chosen",
                self.problems.len(),
                path.display()
            ),
            None => log::debug!("no fullscreen.json; fullscreen stays on the current display"),
        }
        for problem in &self.problems {
            log::warn!("fullscreen: [{}] {}", problem.code, problem.message);
        }
        self.problems.len()
    }
}

/// The monitor picker and the fullscreen toggle.
///
/// `fullscreen` is whether the pop-out is fullscreen now, which is the
/// application's to know: the picker only asks. The chosen display is recorded
/// in `state` as it is clicked, so the caller's only job is to apply the
/// returned action to the pop-out window and to persist the state.
pub fn monitor_picker_ui(
    ui: &mut egui::Ui,
    state: &mut FullscreenState,
    fullscreen: bool,
) -> Option<FullscreenAction> {
    let mut action = None;
    ui.label(PICKER_TITLE);
    if ui
        .selectable_label(state.monitor_index().is_none(), CURRENT_DISPLAY_LABEL)
        .clicked()
    {
        state.choose(None);
        action = Some(FullscreenAction::Choose(None));
    }
    let chosen = state.monitor_index();
    // The rows are collected first: each one borrows the state to paint, and
    // clicking one writes to it.
    let rows: Vec<(usize, String)> = state
        .monitors()
        .as_slice()
        .iter()
        .map(|monitor| (monitor.index, monitor.label()))
        .collect();
    for (index, label) in rows {
        if ui.selectable_label(chosen == Some(index), label).clicked() {
            state.choose(Some(index));
            action = Some(FullscreenAction::Choose(Some(index)));
        }
    }
    // The escape hatch for a display eframe cannot enumerate: its number.
    let mut number = state.display_number();
    ui.add(egui::DragValue::new(&mut number).range(1..=MAX_DISPLAYS));
    if ui.button(USE_NUMBER_LABEL).clicked() {
        state.choose_index(number.saturating_sub(1));
        action = Some(FullscreenAction::Choose(Some(number.saturating_sub(1))));
    }
    let label = if fullscreen { LEAVE_LABEL } else { ENTER_LABEL };
    if ui.button(label).clicked() {
        action = Some(if fullscreen {
            FullscreenAction::Leave
        } else {
            FullscreenAction::Enter
        });
    }
    action
}

#[cfg(test)]
mod tests {
    use super::{
        FULLSCREEN_FILE_NAME, FullscreenSettings, FullscreenState, Monitor, MonitorChoice,
        MonitorList,
    };
    use crate::codes;

    /// A scratch directory of this test's own, emptied first.
    ///
    /// The same shape the dock layout tests use: the process and thread put
    /// two tests in the same binary in different directories.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "subordinate-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Two displays: a laptop panel and the review TV.
    fn two_displays() -> MonitorList {
        MonitorList::new(vec![
            Monitor::new("eDP-1", [1920.0, 1080.0], true),
            Monitor::new("HDMI-1", [3840.0, 2160.0], false),
        ])
    }

    #[test]
    fn a_list_numbers_its_displays_in_platform_order() {
        let monitors = two_displays();
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors.get(1).map(|monitor| monitor.index), Some(1));
        assert_eq!(
            monitors.get(1).map(Monitor::label).as_deref(),
            Some("2: HDMI-1 - 3840x2160"),
            "the row names the number the window system takes, 1-based"
        );
        assert!(
            monitors
                .get(0)
                .is_some_and(|monitor| monitor.label().ends_with("(primary)")),
            "the primary display says so"
        );
    }

    #[test]
    fn nothing_is_chosen_until_the_user_picks() {
        let mut state = FullscreenState::new();
        state.set_monitors(two_displays());
        assert_eq!(state.monitor_index(), None);
        assert_eq!(state.chosen(), None);
    }

    #[test]
    fn choosing_a_display_records_its_index_and_its_name() {
        let mut state = FullscreenState::new();
        state.set_monitors(two_displays());
        state.choose(Some(1));
        assert_eq!(state.monitor_index(), Some(1));
        assert_eq!(state.settings().monitor_name(), Some("HDMI-1"));
        assert_eq!(state.display_number(), 2, "the box follows the choice");
    }

    #[test]
    fn an_unlistable_display_can_still_be_named_by_number() {
        let mut state = FullscreenState::new();
        state.choose_index(1);
        assert_eq!(
            state.monitor_index(),
            Some(1),
            "the number reaches the window system even with no list"
        );
        assert_eq!(state.settings().monitor_name(), Some("Display 2"));
    }

    #[test]
    fn a_replugged_display_is_followed_by_name() {
        let mut state = FullscreenState::new();
        state.set_monitors(two_displays());
        state.choose(Some(1));
        // The TV comes back first in the platform's list after a reboot.
        state.set_monitors(MonitorList::new(vec![
            Monitor::new("HDMI-1", [3840.0, 2160.0], false),
            Monitor::new("eDP-1", [1920.0, 1080.0], true),
        ]));
        assert_eq!(
            state.monitor_index(),
            Some(0),
            "the choice follows the display, not the slot it used to be in"
        );
        assert_eq!(state.settings().monitor_name(), Some("HDMI-1"));
    }

    #[test]
    fn the_choice_round_trips_through_the_settings_file() {
        let mut state = FullscreenState::new();
        state.set_monitors(two_displays());
        state.choose(Some(1));
        let text = state.to_json().expect("the choice serialises");
        let restored = FullscreenSettings::from_json(&text).expect("and reads back");
        assert_eq!(restored, *state.settings());
    }

    #[test]
    fn a_malformed_settings_file_is_reported_not_fatal() {
        let error = FullscreenSettings::from_json("{").expect_err("that is not JSON");
        assert_eq!(error.code, codes::FULLSCREEN_PARSE);

        let newer = serde_json::json!({ "version": 99, "monitor": null }).to_string();
        let error = FullscreenSettings::from_json(&newer).expect_err("another schema version");
        assert_eq!(error.code, codes::FULLSCREEN_PARSE);
    }

    #[test]
    fn the_choice_persists_and_loads_back() {
        let dir = scratch_dir("fullscreen-settings");
        let mut state = FullscreenState::new();
        state.set_monitors(two_displays());
        state.choose(Some(1));
        assert!(state.is_dirty(), "a choice that was never saved is dirty");
        assert!(
            state.persist_to_dir(&dir).expect("the file is written"),
            "the first save writes"
        );
        assert!(!state.is_dirty(), "and the choice is then clean");
        assert!(
            !state
                .persist_to_dir(&dir)
                .expect("no second write is needed"),
            "an unchanged choice writes nothing"
        );

        let loaded = FullscreenState::load_from_dir(&dir);
        assert!(loaded.problems.is_empty(), "the file it just wrote parses");
        assert_eq!(
            loaded.source.as_deref(),
            Some(dir.join(FULLSCREEN_FILE_NAME).as_path())
        );
        assert_eq!(loaded.state.monitor_index(), Some(1));
        assert_eq!(loaded.state.settings().monitor_name(), Some("HDMI-1"));
        assert!(
            !loaded.state.is_dirty(),
            "a freshly loaded choice matches the file"
        );
    }

    #[test]
    fn a_missing_file_is_no_choice_and_no_problem() {
        let dir = scratch_dir("fullscreen-missing");
        let loaded = FullscreenState::load_from_dir(&dir);
        assert_eq!(loaded.state.monitor_index(), None);
        assert_eq!(loaded.source, None);
        assert_eq!(loaded.log_problems(), 0);
    }

    #[test]
    fn a_broken_file_loads_as_no_choice_and_reports() {
        let dir = scratch_dir("fullscreen-broken");
        std::fs::create_dir_all(&dir).expect("the scratch directory is made");
        std::fs::write(dir.join(FULLSCREEN_FILE_NAME), "not json").expect("the fixture is written");
        let loaded = FullscreenState::load_from_dir(&dir);
        assert_eq!(
            loaded.state.monitor_index(),
            None,
            "the editor still starts"
        );
        assert_eq!(loaded.log_problems(), 1);
        assert_eq!(loaded.problems[0].code, codes::FULLSCREEN_PARSE);
    }

    #[test]
    fn a_saved_index_with_no_list_is_kept() {
        let stored = FullscreenSettings {
            monitor: Some(MonitorChoice {
                index: 2,
                name: "DP-3".to_owned(),
            }),
        };
        let text = stored.to_json().expect("the choice serialises");
        let read = FullscreenSettings::from_json(&text).expect("and reads back");
        assert_eq!(
            read.monitor_index(),
            Some(2),
            "a display this run cannot enumerate is still asked for"
        );
    }
}
