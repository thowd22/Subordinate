//! The dockable panel layout: the five editor panels of docs/PLAN.md §5.7,
//! hosted by `egui_dock` and remembered between runs.
//!
//! [`Panel`] names a panel; [`DockLayout`] owns the arrangement of them. The
//! panels can be dragged into other splits and grouped into tabs, so the
//! arrangement is the user's, not the program's: it is written to
//! `layout.json` in the same per-user config directory
//! ([`config_dir`](crate::keymap::config_dir)) that `keymap.toml` comes from,
//! and read back at startup.
//!
//! A layout file is configuration, never a reason to refuse to start. A
//! missing file is the default layout; a malformed or unreadable one is the
//! default layout plus a [`SubError`] in [`LoadedLayout::problems`] for the
//! caller to log. A file that parses but has lost a panel (an older release
//! wrote it, or it was hand-edited) is repaired rather than thrown away: the
//! missing panels come back and everything else stays where the user put it.
//!
//! ```
//! use sub_ui::dock::{DockLayout, Panel};
//!
//! let mut layout = DockLayout::new();
//! assert_eq!(layout.panels().len(), Panel::ALL.len());
//!
//! // The arrangement round-trips through the file format.
//! let json = layout.to_json().expect("the layout serialises");
//! let restored = DockLayout::from_json(&json).expect("and reads back");
//! assert_eq!(restored.panels(), layout.panels());
//! ```

use std::path::{Path, PathBuf};

use eframe::egui::{Id, Ui, WidgetText};
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use serde::{Deserialize, Serialize};
use sub_core::SubError;

use crate::codes;
use crate::keymap::config_dir;

/// The name of the layout file inside the config directory.
pub const LAYOUT_FILE_NAME: &str = "layout.json";

/// The schema version written into the layout file.
///
/// A file from a different version is not migrated: the arrangement is a
/// convenience, so it is dropped for the default layout and reported.
pub const LAYOUT_VERSION: u32 = 1;

/// How much of the window the left column (the media bin) takes.
const BIN_FRACTION: f32 = 0.22;

/// How much of what is left the viewer takes, the rest going to the
/// inspector and export column on the right.
const VIEWER_FRACTION: f32 = 0.72;

/// How much of the window height everything above the timeline takes.
const UPPER_FRACTION: f32 = 0.62;

/// One editor panel.
///
/// The list is exactly the panel list of docs/PLAN.md §5.7, in the order it
/// is written there. The serialised names are stable: they are what a
/// `layout.json` written by an older build contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panel {
    /// The project's media items and bins.
    MediaBin,
    /// The multi-track timeline of the open sequence.
    Timeline,
    /// The preview picture, scrub bar and timecode.
    Viewer,
    /// The parameters of whatever is selected.
    Inspector,
    /// Export presets and render progress.
    Export,
}

impl Panel {
    /// Every panel, in the order docs/PLAN.md §5.7 lists them.
    pub const ALL: [Self; 5] = [
        Self::MediaBin,
        Self::Timeline,
        Self::Viewer,
        Self::Inspector,
        Self::Export,
    ];

    /// The panel's stable identifier, used for its egui id and in logs.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::MediaBin => "panel.media_bin",
            Self::Timeline => "panel.timeline",
            Self::Viewer => "panel.viewer",
            Self::Inspector => "panel.inspector",
            Self::Export => "panel.export",
        }
    }

    /// The panel's tab title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::MediaBin => "Media",
            Self::Timeline => "Timeline",
            Self::Viewer => "Viewer",
            Self::Inspector => "Inspector",
            Self::Export => "Export",
        }
    }
}

/// The layout file: a version tag and the arrangement itself.
#[derive(Serialize, Deserialize)]
struct StoredLayout {
    /// [`LAYOUT_VERSION`] as of the build that wrote the file.
    version: u32,
    /// The docking arrangement.
    dock: DockState<Panel>,
}

/// The arrangement of the editor's panels.
///
/// Wraps `egui_dock`'s [`DockState`] with the two things the editor needs on
/// top of it: a default arrangement, and knowing whether what is on screen
/// still matches what is on disk.
pub struct DockLayout {
    /// The arrangement `egui_dock` draws and the user drags around.
    state: DockState<Panel>,
    /// The JSON last read from or written to disk, so [`DockLayout::persist`]
    /// can tell an untouched layout from a rearranged one without a file
    /// read. `None` until the layout has been either loaded or saved.
    persisted: Option<String>,
}

impl std::fmt::Debug for DockLayout {
    /// The panels and whether the arrangement is unsaved. `egui_dock`'s tree
    /// itself is noise in a test failure.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DockLayout")
            .field("panels", &self.panels())
            .field("dirty", &self.is_dirty())
            .finish()
    }
}

impl Default for DockLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl DockLayout {
    /// The default arrangement: media bin on the left, viewer in the middle,
    /// inspector and export grouped as tabs on the right, timeline across the
    /// bottom.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: default_state(),
            persisted: None,
        }
    }

    /// Reads the layout from the per-user config directory.
    ///
    /// Never fails: whatever went wrong comes back in
    /// [`LoadedLayout::problems`] and the default layout is used.
    #[must_use]
    pub fn load() -> LoadedLayout {
        config_dir().map_or_else(
            || LoadedLayout {
                layout: Self::new(),
                source: None,
                problems: Vec::new(),
            },
            |dir| Self::load_from_dir(&dir),
        )
    }

    /// [`DockLayout::load`] against an arbitrary config directory.
    #[must_use]
    pub fn load_from_dir(dir: &Path) -> LoadedLayout {
        let path = dir.join(LAYOUT_FILE_NAME);
        let mut problems = Vec::new();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return LoadedLayout {
                    layout: Self::new(),
                    source: None,
                    problems,
                };
            }
            Err(error) => {
                problems.push(
                    SubError::wrap(
                        codes::LAYOUT_UNREADABLE,
                        format!("{} could not be read", path.display()),
                        &error,
                    )
                    .with_detail("path", path.display().to_string()),
                );
                return LoadedLayout {
                    layout: Self::new(),
                    source: Some(path),
                    problems,
                };
            }
        };
        match Self::from_json(&text) {
            Ok(mut layout) => {
                if layout.repair() {
                    log::info!("{} was missing panels; they were restored", path.display());
                }
                LoadedLayout {
                    layout,
                    source: Some(path),
                    problems,
                }
            }
            Err(error) => {
                problems.push(error.with_detail("path", path.display().to_string()));
                LoadedLayout {
                    layout: Self::new(),
                    source: Some(path),
                    problems,
                }
            }
        }
    }

    /// Reads a layout from the text of a layout file.
    ///
    /// The layout is used as written, panels and all; [`DockLayout::repair`]
    /// is what puts a missing panel back.
    ///
    /// # Errors
    ///
    /// [`codes::LAYOUT_PARSE`] when the text is not a layout file this build
    /// understands: bad JSON, an unknown panel name, or another schema
    /// version.
    pub fn from_json(text: &str) -> Result<Self, SubError> {
        let stored: StoredLayout = serde_json::from_str(text).map_err(|error| {
            SubError::wrap(codes::LAYOUT_PARSE, "the layout file is malformed", &error)
        })?;
        if stored.version != LAYOUT_VERSION {
            return Err(SubError::new(
                codes::LAYOUT_PARSE,
                format!(
                    "the layout file is version {}, and this build writes version {LAYOUT_VERSION}",
                    stored.version
                ),
            )
            .with_detail("version", stored.version));
        }
        Ok(Self {
            state: stored.dock,
            persisted: Some(text.to_owned()),
        })
    }

    /// The layout as the text of a layout file.
    ///
    /// # Errors
    ///
    /// [`codes::LAYOUT_UNWRITABLE`] if the arrangement cannot be serialised,
    /// which would be a bug in this crate rather than anything the user did.
    pub fn to_json(&self) -> Result<String, SubError> {
        let stored = StoredLayout {
            version: LAYOUT_VERSION,
            dock: self.state.clone(),
        };
        let mut value = serde_json::to_value(&stored).map_err(|error| {
            SubError::wrap(
                codes::LAYOUT_UNWRITABLE,
                "the panel layout could not be serialised",
                &error,
            )
        })?;
        ground_unpainted_rects(&mut value);
        serde_json::to_string_pretty(&value).map_err(|error| {
            SubError::wrap(
                codes::LAYOUT_UNWRITABLE,
                "the panel layout could not be serialised",
                &error,
            )
        })
    }

    /// Whether the arrangement differs from what was last read or written.
    ///
    /// A layout that has never been loaded or saved counts as changed, so a
    /// first run writes the default layout out once.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        match (&self.persisted, self.to_json()) {
            (Some(saved), Ok(current)) => saved != &current,
            _ => true,
        }
    }

    /// Writes the layout to the per-user config directory, if it changed.
    ///
    /// Returns whether a file was written. Called from eframe's periodic save
    /// hook and once on exit, which is why an unchanged layout costs a
    /// serialise and no file I/O.
    ///
    /// # Errors
    ///
    /// [`codes::LAYOUT_UNWRITABLE`] when the directory cannot be created or
    /// the file cannot be written. There being no config directory at all is
    /// not an error: nothing is saved and `false` comes back.
    pub fn persist(&mut self) -> Result<bool, SubError> {
        let Some(dir) = config_dir() else {
            return Ok(false);
        };
        self.persist_to_dir(&dir)
    }

    /// [`DockLayout::persist`] against an arbitrary config directory.
    ///
    /// # Errors
    ///
    /// [`codes::LAYOUT_UNWRITABLE`] when the directory cannot be created or
    /// the file cannot be written.
    pub fn persist_to_dir(&mut self, dir: &Path) -> Result<bool, SubError> {
        let text = self.to_json()?;
        if self.persisted.as_ref() == Some(&text) {
            return Ok(false);
        }
        let path = dir.join(LAYOUT_FILE_NAME);
        std::fs::create_dir_all(dir).map_err(|error| {
            SubError::wrap(
                codes::LAYOUT_UNWRITABLE,
                format!("{} could not be created", dir.display()),
                &error,
            )
            .with_detail("path", dir.display().to_string())
        })?;
        std::fs::write(&path, &text).map_err(|error| {
            SubError::wrap(
                codes::LAYOUT_UNWRITABLE,
                format!("{} could not be written", path.display()),
                &error,
            )
            .with_detail("path", path.display().to_string())
        })?;
        self.persisted = Some(text);
        Ok(true)
    }

    /// Puts the default arrangement back.
    ///
    /// This is what the View menu's "Reset layout" item does. The file is not
    /// rewritten here; the next [`DockLayout::persist`] does that, because the
    /// reset layout now differs from what is on disk.
    pub fn reset(&mut self) {
        self.state = default_state();
    }

    /// Restores panels the layout has lost and drops duplicates of one.
    ///
    /// Returns whether anything changed. A layout written by a build with a
    /// different panel list still describes a usable arrangement, so the
    /// splits the user made are kept and only the panels are put right.
    pub fn repair(&mut self) -> bool {
        let mut changed = false;
        let mut seen = Vec::new();
        self.state.retain_tabs(|panel| {
            if seen.contains(panel) {
                changed = true;
                return false;
            }
            seen.push(*panel);
            true
        });
        for panel in Panel::ALL {
            if !seen.contains(&panel) {
                self.state.push_to_focused_leaf(panel);
                changed = true;
            }
        }
        changed
    }

    /// Every panel in the layout, in the order the tabs are laid out.
    #[must_use]
    pub fn panels(&self) -> Vec<Panel> {
        self.state
            .iter_all_tabs()
            .map(|(_, panel)| *panel)
            .collect()
    }

    /// The arrangement itself, for a caller that needs more of `egui_dock`
    /// than this wrapper exposes.
    #[must_use]
    pub const fn state(&self) -> &DockState<Panel> {
        &self.state
    }

    /// The arrangement, mutably.
    pub const fn state_mut(&mut self) -> &mut DockState<Panel> {
        &mut self.state
    }

    /// Draws every panel, calling `body` for the contents of each one.
    ///
    /// Tabs are draggable and groupable; they have no close button, because a
    /// closed panel would be a panel with no way back and the plan's list is
    /// fixed.
    pub fn ui(&mut self, ui: &mut Ui, body: impl FnMut(&mut Ui, Panel)) {
        let mut viewer = PanelViewer { body };
        DockArea::new(&mut self.state)
            .style(Style::from_egui(ui.style().as_ref()))
            .show_close_buttons(false)
            .show_add_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_inside(ui, &mut viewer);
    }
}

/// A layout as loaded from disk, with whatever the file got wrong.
pub struct LoadedLayout {
    /// The layout in force: the file's, or the default when it could not be
    /// used.
    pub layout: DockLayout,
    /// The file the layout was read from, when there was one.
    pub source: Option<PathBuf>,
    /// Everything wrong with the file. Empty when there was no file.
    pub problems: Vec<SubError>,
}

impl LoadedLayout {
    /// Logs where the layout came from and everything the file got wrong,
    /// and returns how many problems there were.
    pub fn log_problems(&self) -> usize {
        match &self.source {
            Some(path) if self.problems.is_empty() => {
                log::info!("panel layout loaded from {}", path.display());
            }
            Some(path) => log::warn!(
                "{} problem(s) in {}; the default layout is in use",
                self.problems.len(),
                path.display()
            ),
            None => log::debug!("no layout.json; using the default panel layout"),
        }
        for problem in &self.problems {
            log::warn!("layout: [{}] {}", problem.code, problem.message);
        }
        self.problems.len()
    }
}

/// Bridges `egui_dock`'s tab viewer to a closure that draws one panel.
struct PanelViewer<F> {
    /// Draws the body of whichever panel it is handed.
    body: F,
}

impl<F: FnMut(&mut Ui, Panel)> TabViewer for PanelViewer<F> {
    type Tab = Panel;

    fn id(&mut self, tab: &mut Self::Tab) -> Id {
        Id::new(tab.id())
    }

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        (self.body)(ui, *tab);
    }
}

/// Rewrites the rectangles of nodes that have never been painted so the
/// layout is valid JSON.
///
/// A node `egui_dock` has not laid out yet carries a NaN rectangle, and JSON
/// has no NaN: `serde_json` writes one as `null` and then refuses to read it
/// back as an `f32`, which would make every layout file unreadable. The
/// rectangles are recomputed from the split fractions on the first painted
/// frame, so writing zero costs nothing. Only `rect` and `viewport` are
/// touched; a `null` anywhere else is a real `None` and is left alone.
fn ground_unpainted_rects(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if matches!(key.as_str(), "rect" | "viewport") {
                    zero_nulls(child);
                } else {
                    ground_unpainted_rects(child);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                ground_unpainted_rects(item);
            }
        }
        _ => {}
    }
}

/// Replaces every `null` inside a rectangle with zero.
fn zero_nulls(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Null => *value = serde_json::Value::from(0.0),
        serde_json::Value::Object(map) => {
            for child in map.values_mut() {
                zero_nulls(child);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                zero_nulls(item);
            }
        }
        _ => {}
    }
}

/// The default arrangement of docs/PLAN.md §5.7's panels.
fn default_state() -> DockState<Panel> {
    let mut state = DockState::new(vec![Panel::Viewer]);
    let surface = state.main_surface_mut();
    let [upper, _timeline] =
        surface.split_below(NodeIndex::root(), UPPER_FRACTION, vec![Panel::Timeline]);
    let [middle, _bin] = surface.split_left(upper, BIN_FRACTION, vec![Panel::MediaBin]);
    surface.split_right(
        middle,
        VIEWER_FRACTION,
        vec![Panel::Inspector, Panel::Export],
    );
    state
}

/// The View menu's layout items. Returns whether the layout was reset.
///
/// Drawn inside a menu; the caller supplies the menu itself so this sits
/// among the editor's other view items.
pub fn layout_menu_ui(ui: &mut Ui, layout: &mut DockLayout) -> bool {
    let clicked = ui.button("Reset layout").clicked();
    if clicked {
        layout.reset();
        ui.close();
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::{DockLayout, LAYOUT_FILE_NAME, LAYOUT_VERSION, Panel, default_state};
    use crate::codes;

    /// The panels of a layout, sorted, for comparing sets rather than order.
    fn panel_set(layout: &DockLayout) -> Vec<Panel> {
        let mut panels = layout.panels();
        panels.sort_unstable();
        panels
    }

    #[test]
    fn the_default_layout_is_the_plans_panel_list() {
        let layout = DockLayout::new();
        let mut expected = Panel::ALL.to_vec();
        expected.sort_unstable();
        assert_eq!(
            panel_set(&layout),
            expected,
            "every panel of docs/PLAN.md 5.7 is in the default layout, exactly once"
        );
    }

    #[test]
    fn the_default_layout_groups_the_inspector_and_export_as_tabs() {
        let state = default_state();
        let grouped = state.main_surface().iter().any(|node| {
            node.tabs().is_some_and(|tabs| {
                tabs.contains(&Panel::Inspector) && tabs.contains(&Panel::Export)
            })
        });
        assert!(grouped, "the inspector and export panels share a tab group");
    }

    #[test]
    fn a_layout_round_trips_through_its_file_format() {
        let layout = DockLayout::new();
        let json = layout.to_json().expect("the default layout serialises");
        let restored = DockLayout::from_json(&json).expect("and reads back");
        assert_eq!(restored.panels(), layout.panels());
        assert_eq!(
            restored.to_json().expect("re-serialises"),
            json,
            "a round trip is byte-for-byte stable"
        );
    }

    #[test]
    fn an_unpainted_layout_writes_no_nan_rectangles() {
        // A layout saved before its first paint still has to read back:
        // NaN would have been written as a `null` no `f32` accepts.
        let json = DockLayout::new().to_json().expect("serialises");
        assert!(
            !json.contains("\"x\": null"),
            "an unpainted rectangle is written as zero, not null: {json}"
        );
        DockLayout::from_json(&json).expect("and reads back");
    }

    #[test]
    fn a_freshly_read_layout_is_not_dirty() {
        let json = DockLayout::new().to_json().expect("serialises");
        let layout = DockLayout::from_json(&json).expect("reads back");
        assert!(!layout.is_dirty(), "nothing has moved since it was read");
    }

    #[test]
    fn a_layout_that_has_never_been_saved_is_dirty() {
        assert!(
            DockLayout::new().is_dirty(),
            "the first run writes the default layout out once"
        );
    }

    #[test]
    fn resetting_restores_the_default_arrangement() {
        let mut layout = DockLayout::new();
        let default = layout.to_json().expect("serialises");
        layout.state_mut().push_to_focused_leaf(Panel::Viewer);
        assert_ne!(
            layout.to_json().expect("serialises"),
            default,
            "the layout was changed"
        );
        layout.reset();
        assert_eq!(
            layout.to_json().expect("serialises"),
            default,
            "reset puts the default back"
        );
    }

    #[test]
    fn malformed_json_is_reported_rather_than_fatal() {
        let error = DockLayout::from_json("{ not json").expect_err("malformed");
        assert_eq!(error.code, codes::LAYOUT_PARSE);
    }

    #[test]
    fn an_unknown_panel_name_is_reported() {
        let json = format!(r#"{{"version":{LAYOUT_VERSION},"dock":{{"surfaces":[]}}}}"#);
        // A well-formed envelope with a dock this build cannot read.
        let broken = json.replace("\"surfaces\":[]", "\"surfaces\":[\"nonsense\"]");
        let error = DockLayout::from_json(&broken).expect_err("unreadable dock state");
        assert_eq!(error.code, codes::LAYOUT_PARSE);
    }

    #[test]
    fn another_schema_version_is_reported() {
        let json = DockLayout::new().to_json().expect("serialises");
        let future = json.replace(
            &format!("\"version\": {LAYOUT_VERSION}"),
            &format!("\"version\": {}", LAYOUT_VERSION + 1),
        );
        let error = DockLayout::from_json(&future).expect_err("a version we do not write");
        assert_eq!(error.code, codes::LAYOUT_PARSE);
    }

    #[test]
    fn a_layout_missing_a_panel_is_repaired() {
        let mut layout = DockLayout::new();
        let path = layout
            .state()
            .find_tab(&Panel::Export)
            .expect("the default layout has an export panel");
        layout.state_mut().remove_tab(path);
        assert!(!layout.panels().contains(&Panel::Export));

        assert!(layout.repair(), "the missing panel is put back");
        assert_eq!(
            panel_set(&layout),
            {
                let mut all = Panel::ALL.to_vec();
                all.sort_unstable();
                all
            },
            "every panel is present again"
        );
        assert!(!layout.repair(), "a complete layout is left alone");
    }

    #[test]
    fn a_duplicated_panel_is_dropped() {
        let mut layout = DockLayout::new();
        layout.state_mut().push_to_focused_leaf(Panel::Viewer);
        assert!(layout.repair(), "the duplicate is dropped");
        assert_eq!(
            layout
                .panels()
                .iter()
                .filter(|p| **p == Panel::Viewer)
                .count(),
            1,
            "one viewer is left"
        );
    }

    #[test]
    fn a_missing_file_is_the_default_layout_and_no_problem() {
        let dir =
            std::env::temp_dir().join(format!("subordinate-layout-missing-{}", std::process::id()));
        let loaded = DockLayout::load_from_dir(&dir);
        assert!(loaded.problems.is_empty(), "a first run is not a problem");
        assert!(loaded.source.is_none());
        assert_eq!(panel_set(&loaded.layout), {
            let mut all = Panel::ALL.to_vec();
            all.sort_unstable();
            all
        });
    }

    #[test]
    fn a_saved_layout_is_read_back_and_a_broken_one_is_reported() {
        let dir = std::env::temp_dir().join(format!(
            "subordinate-layout-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        let mut layout = DockLayout::new();
        layout.state_mut().push_to_focused_leaf(Panel::Viewer);
        layout.repair();
        assert!(
            layout.persist_to_dir(&dir).expect("the layout is written"),
            "the first save writes the file"
        );
        assert!(
            !layout.persist_to_dir(&dir).expect("no second write"),
            "an unchanged layout is not rewritten"
        );

        let loaded = DockLayout::load_from_dir(&dir);
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        assert_eq!(loaded.layout.panels(), layout.panels());
        assert!(!loaded.layout.is_dirty());

        std::fs::write(dir.join(LAYOUT_FILE_NAME), "{ not json").expect("the file is replaced");
        let broken = DockLayout::load_from_dir(&dir);
        assert_eq!(broken.problems.len(), 1);
        assert_eq!(broken.problems[0].code, codes::LAYOUT_PARSE);
        assert_eq!(
            panel_set(&broken.layout),
            {
                let mut all = Panel::ALL.to_vec();
                all.sort_unstable();
                all
            },
            "a broken file falls back to the default layout"
        );
        assert_eq!(broken.log_problems(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_where_the_file_should_be_is_reported_as_unreadable() {
        let dir = std::env::temp_dir().join(format!(
            "subordinate-layout-unreadable-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(LAYOUT_FILE_NAME)).expect("a directory in its place");
        let loaded = DockLayout::load_from_dir(&dir);
        assert_eq!(loaded.problems.len(), 1);
        assert_eq!(loaded.problems[0].code, codes::LAYOUT_UNREADABLE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn panel_ids_and_titles_are_distinct() {
        let mut ids: Vec<_> = Panel::ALL.iter().map(|panel| panel.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Panel::ALL.len(), "every panel has its own id");
        let mut titles: Vec<_> = Panel::ALL.iter().map(|panel| panel.title()).collect();
        titles.sort_unstable();
        titles.dedup();
        assert_eq!(titles.len(), Panel::ALL.len());
    }
}
