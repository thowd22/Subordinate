//! The plugins panel: what is installed, what it asks for, and what went
//! wrong.
//!
//! Agents manage plugins through `plugin.*` on the Command API and developers
//! through the CLI, but a user with neither still needs to see the list and
//! switch something off (docs/PLAN.md §6.4). This module is that view: it
//! reads a [`Scan`] of the two install directories and, when the editor is
//! hot-reloading dev installs, the [`ReloadStatus`] rows the
//! [`DevHost`](sub_plugin::dev::DevHost) keeps, and folds them into one row
//! per plugin.
//!
//! Nothing here mutates anything. A click becomes a [`PluginAction`] the
//! caller performs — through [`PluginAction::perform`], which is the same
//! [`PluginRegistry`] the CLI and the Command API call — so the panel cannot
//! disagree with the other two ways in.
//!
//! Errors are shown where they happened rather than in a log: a directory
//! that would not scan is a row of its own, and a plugin whose last load or
//! hot reload failed keeps its row and carries the failure under it, with the
//! stable code beside the message so a bug report names it.
//!
//! ```
//! use sub_plugin::manifest::Manifest;
//! use sub_plugin::registry::{InstallLocation, InstalledPlugin};
//! use sub_ui::plugins_panel::{LoadStatus, PluginRow};
//!
//! let manifest = Manifest::parse(
//!     "[plugin]\nid = \"com.example.cutter\"\nname = \"Cutter\"\n\
//!      version = \"1.2.3\"\napi = \"0.1\"\nworlds = [\"command\"]\n\
//!      [capabilities]\nfs_read = [\"$PROJECT\"]\n",
//! )
//! .unwrap();
//! let plugin = InstalledPlugin {
//!     id: manifest.plugin.id.clone(),
//!     location: InstallLocation::User,
//!     directory: "/plugins/cutter".into(),
//!     enabled: true,
//!     dev: false,
//!     manifest,
//! };
//!
//! // No reload status: the plugin is installed but this process has not
//! // loaded it, which is not a failure.
//! let row = PluginRow::new(&plugin, None);
//! assert_eq!(row.title(), "Cutter 1.2.3");
//! assert_eq!(row.worlds_label(), "command");
//! assert_eq!(row.capabilities_label(), "read $PROJECT");
//! assert_eq!(row.status, LoadStatus::NotLoaded);
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Ui};
use serde_json::Value;
use sub_core::{SubError, SubResult};
use sub_plugin::dev::{ReloadError, ReloadStatus};
use sub_plugin::manifest::{Capabilities, PluginId, World};
use sub_plugin::registry::{
    EnableChange, InstallLocation, InstalledPlugin, LoadFailure, PluginRegistry, Removal, Scan,
};

use crate::codes;

/// The panel's window title.
pub const PANEL_TITLE: &str = "Plugins";

/// What the list shows when nothing is installed.
pub const EMPTY_LABEL: &str = "No plugins installed";

/// What the capabilities column shows for a plugin that asks for nothing
/// beyond the bare sandbox.
pub const NO_CAPABILITIES: &str = "sandboxed";

/// How a plugin's last load went.
///
/// A plugin the editor never tried to load — because it is switched off, or
/// because this process does not load plugins at all — is not an error, so
/// the two "no load" cases are their own variants rather than a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadStatus {
    /// Switched off, so it was never loaded.
    Disabled,
    /// Enabled, but no load has been attempted in this process yet.
    NotLoaded,
    /// The last load succeeded, and this is what it contributed.
    Loaded {
        /// How many times it has loaded in this process. A dev install's
        /// count going up is how a developer knows the rebuild landed.
        generation: u64,
        /// Menu commands the loaded version registers.
        commands: usize,
        /// Effects it declares.
        effects: usize,
        /// MCP tools it contributes.
        tools: usize,
    },
    /// The last load or hot reload failed. Whatever loaded before is still
    /// the version answering, which is what `generation` counts.
    Failed {
        /// Successful loads so far; zero when it has never loaded.
        generation: u64,
        /// Why the last attempt failed.
        error: ReloadError,
    },
}

impl LoadStatus {
    /// The short word the status column shows.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NotLoaded => "not loaded",
            Self::Loaded { .. } => "loaded",
            Self::Failed { .. } => "failed",
        }
    }

    /// Whether the last attempt to load failed.
    #[must_use]
    pub const fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    /// The failure, when there is one.
    #[must_use]
    pub const fn error(&self) -> Option<&ReloadError> {
        match self {
            Self::Failed { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// One installed plugin as the panel shows it.
///
/// Everything is taken from the manifest the user approved on install plus
/// the load status, so a row can be drawn without reading a file or
/// instantiating a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRow {
    /// The plugin's id, which is the key every action names.
    pub id: PluginId,
    /// The display name from its manifest.
    pub name: String,
    /// Its own version.
    pub version: String,
    /// Which of the two install directories it was found in.
    pub location: InstallLocation,
    /// The directory holding its `plugin.toml`, which is what the open-folder
    /// action opens.
    pub directory: PathBuf,
    /// Whether it is switched on.
    pub enabled: bool,
    /// Whether it is a `--dev` install, linked to a source tree and watched.
    pub dev: bool,
    /// The WIT worlds its manifest declares.
    pub worlds: Vec<World>,
    /// What it asked the sandbox for.
    pub capabilities: Capabilities,
    /// How its last load went.
    pub status: LoadStatus,
}

impl PluginRow {
    /// The row for one installed plugin, given the reload status the host
    /// holds for it, if any.
    #[must_use]
    pub fn new(plugin: &InstalledPlugin, status: Option<&ReloadStatus>) -> Self {
        Self {
            id: plugin.id.clone(),
            name: plugin.name().to_owned(),
            version: plugin.version(),
            location: plugin.location,
            directory: plugin.directory.clone(),
            enabled: plugin.enabled,
            dev: plugin.dev || status.is_some_and(|status| status.dev),
            worlds: plugin.manifest.plugin.worlds.clone(),
            capabilities: plugin.manifest.capabilities.clone(),
            status: load_status(plugin, status),
        }
    }

    /// The worlds as one comma-separated line.
    #[must_use]
    pub fn worlds_label(&self) -> String {
        self.worlds
            .iter()
            .map(|world| world.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// What the plugin asked the sandbox for, as one line.
    ///
    /// Filesystem roots are named rather than counted: which directory a
    /// plugin may read is the whole point of the approval.
    #[must_use]
    pub fn capabilities_label(&self) -> String {
        let capabilities = &self.capabilities;
        if capabilities.is_empty() {
            return NO_CAPABILITIES.to_owned();
        }
        let mut parts = Vec::new();
        if !capabilities.fs_read.is_empty() {
            parts.push(format!("read {}", capabilities.fs_read.join(" ")));
        }
        if !capabilities.fs_write.is_empty() {
            parts.push(format!("write {}", capabilities.fs_write.join(" ")));
        }
        if capabilities.network {
            parts.push("network".to_owned());
        }
        if capabilities.shaders {
            parts.push("shaders".to_owned());
        }
        parts.join(", ")
    }

    /// The heading line: name, version and, for a dev install, that it is one.
    #[must_use]
    pub fn title(&self) -> String {
        if self.dev {
            format!("{} {} (dev)", self.name, self.version)
        } else {
            format!("{} {}", self.name, self.version)
        }
    }
}

/// How `plugin` last loaded, from the status the host kept for it.
fn load_status(plugin: &InstalledPlugin, status: Option<&ReloadStatus>) -> LoadStatus {
    let Some(status) = status else {
        return if plugin.enabled {
            LoadStatus::NotLoaded
        } else {
            LoadStatus::Disabled
        };
    };
    if let Some(error) = &status.error {
        return LoadStatus::Failed {
            generation: status.generation,
            error: error.clone(),
        };
    }
    if !plugin.enabled {
        return LoadStatus::Disabled;
    }
    LoadStatus::Loaded {
        generation: status.generation,
        commands: status.commands,
        effects: status.effects,
        tools: status.tools,
    }
}

/// What a click in the panel asks for.
///
/// The panel raises these and performs none of them, the way every other
/// panel raises actions: the caller owns the registry, and the same call is
/// what `plugin.enable`, `plugin.disable` and `plugin.remove` make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginAction {
    /// Switch a plugin on.
    Enable(PluginId),
    /// Switch a plugin off. It stays installed.
    Disable(PluginId),
    /// Delete a plugin's directory.
    Remove(PluginId),
    /// Show a plugin's directory in the platform's file manager.
    OpenFolder(PathBuf),
}

/// What performing a [`PluginAction`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginOutcome {
    /// A plugin was switched on or off.
    Switched(EnableChange),
    /// A plugin's directory was deleted.
    Removed(Removal),
    /// A directory was handed to the platform's file manager.
    Opened(PathBuf),
}

impl PluginAction {
    /// Performs the action against `registry`.
    ///
    /// # Errors
    ///
    /// Whatever [`PluginRegistry::set_enabled`] or [`PluginRegistry::remove`]
    /// returns — `plugin.not_installed` for an id the registry no longer
    /// knows, which is what a panel one scan out of date raises — and
    /// [`codes::PLUGIN_FOLDER_UNOPENABLE`] when the platform's file manager
    /// could not be started.
    pub fn perform(&self, registry: &PluginRegistry) -> SubResult<PluginOutcome> {
        match self {
            Self::Enable(id) => registry.set_enabled(id, true).map(PluginOutcome::Switched),
            Self::Disable(id) => registry.set_enabled(id, false).map(PluginOutcome::Switched),
            Self::Remove(id) => registry.remove(id).map(PluginOutcome::Removed),
            Self::OpenFolder(path) => {
                open_folder(path)?;
                Ok(PluginOutcome::Opened(path.clone()))
            }
        }
    }
}

/// Hands `path` to the platform's file manager.
///
/// # Errors
///
/// [`codes::PLUGIN_FOLDER_UNOPENABLE`] when the opener cannot be started,
/// which on a headless machine it usually cannot. Failing to open a folder is
/// never worth more than a message: the path is in the row above it.
pub fn open_folder(path: &Path) -> SubResult<()> {
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|err| {
            SubError::wrap(
                codes::PLUGIN_FOLDER_UNOPENABLE,
                "the plugin folder could not be opened",
                &err,
            )
            .with_detail("path", path.display().to_string())
            .with_detail("opener", program)
        })
}

/// The plugins panel: one row per installed plugin, plus a row per directory
/// that would not scan.
#[derive(Debug, Clone, Default)]
pub struct PluginsPanel {
    /// Whether the window is showing.
    pub open: bool,
    /// The installed plugins, in the scan's id order.
    rows: Vec<PluginRow>,
    /// The directories that failed to load, in the scan's path order.
    failures: Vec<LoadFailure>,
}

impl PluginsPanel {
    /// A closed panel with nothing in it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the panel if it is closed, and closes it if it is open.
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Rebuilds the list from a scan and the host's reload statuses.
    ///
    /// Call it after anything that changes what is installed: a scan, an
    /// install, a removal, an enable or a hot reload. `statuses` may be empty
    /// in a process that lists plugins without loading them, and every row is
    /// then simply "not loaded".
    pub fn sync(&mut self, scan: &Scan, statuses: &[ReloadStatus]) {
        let by_id: BTreeMap<&PluginId, &ReloadStatus> =
            statuses.iter().map(|status| (&status.id, status)).collect();
        self.rows = scan
            .plugins()
            .iter()
            .map(|plugin| PluginRow::new(plugin, by_id.get(&plugin.id).copied()))
            .collect();
        self.failures = scan.failures().to_vec();
    }

    /// The list built straight from rows, for tests and for a caller that
    /// keeps its scan on another thread.
    pub fn set_rows(&mut self, rows: Vec<PluginRow>, failures: Vec<LoadFailure>) {
        self.rows = rows;
        self.failures = failures;
    }

    /// The installed plugins.
    #[must_use]
    pub fn rows(&self) -> &[PluginRow] {
        &self.rows
    }

    /// The directories that would not scan.
    #[must_use]
    pub fn failures(&self) -> &[LoadFailure] {
        &self.failures
    }

    /// The row for one plugin, if it is installed.
    #[must_use]
    pub fn row(&self, id: &PluginId) -> Option<&PluginRow> {
        self.rows.iter().find(|row| row.id == *id)
    }

    /// Whether anything at all is installed or broken.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.failures.is_empty()
    }

    /// Whether anything on the list needs attention: a directory that would
    /// not scan, or a plugin whose last load failed.
    #[must_use]
    pub fn has_problems(&self) -> bool {
        !self.failures.is_empty() || self.rows.iter().any(|row| row.status.is_failed())
    }

    /// Draws the panel as a window, if it is open.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<PluginAction> {
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut action = None;
        egui::Window::new(PANEL_TITLE)
            .open(&mut open)
            .resizable(true)
            .default_width(520.0)
            .show(ctx, |ui| {
                action = self.ui(ui);
            });
        self.open = open;
        action
    }

    /// Draws the list into an existing layout, so it can be docked or
    /// exercised without a window.
    pub fn ui(&mut self, ui: &mut Ui) -> Option<PluginAction> {
        let mut action = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.rows.is_empty() && self.failures.is_empty() {
                    ui.weak(EMPTY_LABEL);
                }
                for row in &self.rows {
                    if let Some(chosen) = plugin_row_ui(ui, row) {
                        action = Some(chosen);
                    }
                    ui.separator();
                }
                for failure in &self.failures {
                    failure_row_ui(ui, failure);
                    ui.separator();
                }
            });
        action
    }
}

/// One installed plugin: its heading, what it is, what it asks for, its
/// actions, and its failure when it has one.
fn plugin_row_ui(ui: &mut Ui, row: &PluginRow) -> Option<PluginAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        let title = if row.enabled {
            egui::RichText::new(row.title()).strong()
        } else {
            egui::RichText::new(row.title()).weak()
        };
        ui.label(title);
        ui.label(status_text(&row.status));
    });
    ui.label(egui::RichText::new(row.id.as_str()).weak());
    ui.label(format!(
        "worlds: {}  |  capabilities: {}  |  {}",
        row.worlds_label(),
        row.capabilities_label(),
        row.location
    ));
    if let LoadStatus::Loaded {
        commands,
        effects,
        tools,
        generation,
    } = row.status
    {
        ui.label(egui::RichText::new(format!(
            "{commands} commands, {effects} effects, {tools} tools (load {generation})"
        )));
    }
    if let Some(error) = row.status.error() {
        error_ui(ui, &error.code, &error.message, &error.details);
    }
    ui.horizontal(|ui| {
        if row.enabled {
            if ui.button("Disable").clicked() {
                action = Some(PluginAction::Disable(row.id.clone()));
            }
        } else if ui.button("Enable").clicked() {
            action = Some(PluginAction::Enable(row.id.clone()));
        }
        if ui.button("Remove").clicked() {
            action = Some(PluginAction::Remove(row.id.clone()));
        }
        if ui.button("Open folder").clicked() {
            action = Some(PluginAction::OpenFolder(row.directory.clone()));
        }
    });
    action
}

/// The status word, coloured when it is one worth noticing.
fn status_text(status: &LoadStatus) -> egui::RichText {
    let text = egui::RichText::new(status.label());
    match status {
        LoadStatus::Failed { .. } => text.color(FAILED_COLOR),
        LoadStatus::Disabled | LoadStatus::NotLoaded => text.weak(),
        LoadStatus::Loaded { .. } => text,
    }
}

/// A directory that would not scan: there is no plugin to act on, so the row
/// is the path and the failure.
fn failure_row_ui(ui: &mut Ui, failure: &LoadFailure) {
    ui.label(
        egui::RichText::new(failure.directory.display().to_string())
            .strong()
            .color(FAILED_COLOR),
    );
    ui.label(egui::RichText::new(failure.location.to_string()).weak());
    error_ui(ui, &failure.code, &failure.message, &failure.details);
}

/// One failure, inline: its message, its stable code, and its details.
///
/// The code is shown rather than hidden because it is what a bug report and
/// the docs are indexed by, and the `hint` detail the error catalogue fills
/// in is what tells the user what to do next.
fn error_ui(ui: &mut Ui, code: &str, message: &str, details: &BTreeMap<String, Value>) {
    ui.label(egui::RichText::new(message).color(FAILED_COLOR));
    ui.label(egui::RichText::new(code).weak().monospace());
    for (key, value) in details {
        ui.label(egui::RichText::new(format!("{key}: {}", detail_text(value))).weak());
    }
}

/// A detail value as one line: a string as itself, anything else as JSON.
fn detail_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The colour a failure is drawn in, matching egui's own error red closely
/// enough to read on both themes.
const FAILED_COLOR: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x60, 0x60);

#[cfg(test)]
mod tests {
    use sub_plugin::manifest::Manifest;

    use super::*;

    /// An installed plugin built from a manifest, with no dev link.
    fn installed(id: &str, enabled: bool) -> InstalledPlugin {
        let manifest = Manifest::parse(&format!(
            "[plugin]\nid = \"{id}\"\nname = \"Cutter\"\nversion = \"1.2.3\"\n\
             api = \"0.1\"\nworlds = [\"command\", \"effect\"]\n\
             [capabilities]\nfs_read = [\"$PROJECT\"]\nshaders = true\n"
        ))
        .expect("a valid manifest");
        InstalledPlugin {
            id: manifest.plugin.id.clone(),
            location: InstallLocation::User,
            directory: PathBuf::from("/plugins").join(id),
            enabled,
            dev: false,
            manifest,
        }
    }

    fn status(id: &PluginId, error: Option<ReloadError>) -> ReloadStatus {
        ReloadStatus {
            id: id.clone(),
            generation: 2,
            ok: error.is_none(),
            dev: true,
            commands: 3,
            effects: 1,
            tools: 0,
            error,
        }
    }

    fn reload_error() -> ReloadError {
        ReloadError {
            code: "plugin.load_failed".to_owned(),
            message: "the component does not instantiate".to_owned(),
            details: BTreeMap::from([("hint".to_owned(), Value::String("rebuild it".to_owned()))]),
        }
    }

    #[test]
    fn a_row_carries_the_version_worlds_and_capabilities_from_the_manifest() {
        let plugin = installed("com.example.cutter", true);
        let row = PluginRow::new(&plugin, None);

        assert_eq!(row.version, "1.2.3");
        assert_eq!(row.worlds_label(), "command, effect");
        assert_eq!(row.capabilities_label(), "read $PROJECT, shaders");
        assert_eq!(row.title(), "Cutter 1.2.3");
        assert_eq!(row.status, LoadStatus::NotLoaded);
    }

    #[test]
    fn a_plugin_asking_for_nothing_says_so_rather_than_showing_an_empty_column() {
        let mut plugin = installed("com.example.cutter", true);
        plugin.manifest.capabilities = Capabilities::default();
        let row = PluginRow::new(&plugin, None);
        assert_eq!(row.capabilities_label(), NO_CAPABILITIES);
    }

    #[test]
    fn a_switched_off_plugin_is_disabled_rather_than_not_loaded() {
        let plugin = installed("com.example.cutter", false);
        let row = PluginRow::new(&plugin, None);
        assert_eq!(row.status, LoadStatus::Disabled);
        assert_eq!(row.status.label(), "disabled");
        assert!(!row.status.is_failed());
    }

    #[test]
    fn a_loaded_plugin_shows_what_it_contributed() {
        let plugin = installed("com.example.cutter", true);
        let row = PluginRow::new(&plugin, Some(&status(&plugin.id, None)));
        assert_eq!(
            row.status,
            LoadStatus::Loaded {
                generation: 2,
                commands: 3,
                effects: 1,
                tools: 0,
            }
        );
        assert!(row.dev, "the status says it is a dev install");
        assert_eq!(row.title(), "Cutter 1.2.3 (dev)");
    }

    #[test]
    fn a_failed_reload_keeps_the_row_and_carries_the_error() {
        let plugin = installed("com.example.cutter", true);
        let row = PluginRow::new(&plugin, Some(&status(&plugin.id, Some(reload_error()))));

        assert!(row.status.is_failed());
        assert_eq!(row.status.label(), "failed");
        let error = row.status.error().expect("the failure is on the row");
        assert_eq!(error.code, "plugin.load_failed");
        assert_eq!(
            error.details["hint"],
            Value::String("rebuild it".to_owned())
        );
    }

    #[test]
    fn an_empty_scan_leaves_an_empty_list() {
        let mut panel = PluginsPanel::new();
        panel.sync(&Scan::default(), &[]);
        assert!(panel.is_empty());
        assert!(!panel.has_problems());
        assert!(panel.rows().is_empty());
        assert!(panel.failures().is_empty());
    }

    #[test]
    fn a_closed_panel_opens_and_closes_again() {
        let mut panel = PluginsPanel::new();
        assert!(!panel.open);
        panel.toggle();
        assert!(panel.open);
        panel.toggle();
        assert!(!panel.open);
        assert!(panel.is_empty());
    }

    #[test]
    fn a_detail_string_reads_as_itself_and_anything_else_as_json() {
        assert_eq!(detail_text(&Value::String("a/b".to_owned())), "a/b");
        assert_eq!(detail_text(&Value::from(7)), "7");
    }
}
