//! The media bin panel: the project's assets, the folders they are filed in,
//! and the two ways of looking at them (docs/PLAN.md §2).
//!
//! The bin is the entry point for every asset, so it is the one panel that
//! starts work outside the project: a native file dialog or a drop from the
//! desktop turns into an [`MediaBinAction::Import`], which the host feeds to
//! an [`ImportQueue`](crate::media_import::ImportQueue). Hashing, probing and
//! thumbnails all happen there, as jobs; the panel itself never blocks and
//! never touches a file.
//!
//! Like every other panel here, it owns no project state and mutates nothing.
//! Creating a folder, renaming one, moving one, filing an item in another
//! folder and relinking an offline item all leave as [`MediaBinAction`]
//! values, each of which maps onto exactly one `sub-edit` command, so every
//! change the user makes in the bin is undoable. What the panel does own is
//! what looking at the bin means and the model does not store: which folder is
//! open, which item is selected, which folders are collapsed, whether the
//! contents are listed or tiled, and which column they are sorted by.
//!
//! Two things about an item are shown that are not metadata: whether its file
//! is missing, and whether a proxy stands in for it while editing. Both are
//! badges on the row and the tile, and the proxy badge names every state —
//! none, generating, ready, stale, failed — so "is this cut running on
//! proxies?" is answered by looking (TASK-70).
//!
//! Durations are [`RationalTime`] end to end. The duration column is a
//! timecode at the item's own rate and the frame-rate column is derived from
//! the exact [`Rational`] by integer arithmetic, so 24000/1001 reads as
//! `23.976` without a float ever existing.

use std::collections::BTreeSet;
use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText, Ui, Vec2};
use sub_edit::BoxedCommand;
use sub_edit::commands::{CreateBin, MoveBin, MoveToBin, RenameBin};
use sub_model::media::StreamInfo;
use sub_model::{Bin, BinId, MediaId, MediaItem, Project, ProxyState};
use sub_time::{Rational, RationalTime, Timecode, TimecodeRate};

/// Shown in a metadata column the item has no answer for.
pub const UNKNOWN: &str = "-";

/// Width of the folder tree column, in points.
const TREE_WIDTH: f32 = 190.0;

/// How far one level of nesting indents the tree, in points.
const INDENT: f32 = 14.0;

/// The size of one grid tile, in points.
const TILE: Vec2 = Vec2::new(132.0, 96.0);

/// How the contents of a bin are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BinViewMode {
    /// One row per item, with the metadata columns.
    #[default]
    List,
    /// A wrapping grid of tiles.
    Grid,
}

impl BinViewMode {
    /// The label of the button that switches to the other mode.
    #[must_use]
    pub fn other(self) -> Self {
        match self {
            Self::List => Self::Grid,
            Self::Grid => Self::List,
        }
    }

    /// The name of this mode, as the toggle button shows it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Grid => "Grid",
        }
    }
}

/// A column of the list view, and the key the contents can be sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortColumn {
    /// Display name, compared case-insensitively.
    #[default]
    Name,
    /// Total duration, compared exactly as a length in seconds.
    Duration,
    /// Frame rate of the first video stream, compared exactly.
    FrameRate,
    /// Pixel count of the first video stream, then width.
    Resolution,
}

impl SortColumn {
    /// Every column, in the order the list view draws them.
    pub const ALL: [Self; 4] = [
        Self::Name,
        Self::Duration,
        Self::FrameRate,
        Self::Resolution,
    ];

    /// The column heading.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Duration => "Duration",
            Self::FrameRate => "FPS",
            Self::Resolution => "Resolution",
        }
    }

    /// How wide the column is drawn, in points.
    #[must_use]
    pub fn width(self) -> f32 {
        match self {
            Self::Name => 220.0,
            Self::FrameRate => 64.0,
            Self::Duration | Self::Resolution => 100.0,
        }
    }

    /// What this column shows for `item`.
    #[must_use]
    pub fn text(self, item: &MediaItem) -> String {
        match self {
            Self::Name => item.name.clone(),
            Self::Duration => duration_text(item),
            Self::FrameRate => frame_rate_text(item),
            Self::Resolution => resolution_text(item),
        }
    }
}

/// Which column the contents are ordered by, and which way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BinSort {
    /// The key.
    pub column: SortColumn,
    /// Ascending when true: A first, shortest first, slowest first, smallest
    /// first.
    pub ascending: bool,
}

impl Default for BinSort {
    /// Ascending by name, which is how a folder opens.
    fn default() -> Self {
        Self::by(SortColumn::default())
    }
}

impl BinSort {
    /// Ascending by `column`.
    #[must_use]
    pub fn by(column: SortColumn) -> Self {
        Self {
            column,
            ascending: true,
        }
    }

    /// What clicking `column`'s heading does: a click on the column already
    /// sorted by reverses it, a click on another one sorts by it, ascending.
    pub fn click(&mut self, column: SortColumn) {
        if self.column == column {
            self.ascending = !self.ascending;
        } else {
            *self = Self::by(column);
        }
    }

    /// The arrow drawn after the heading of the column in force.
    #[must_use]
    pub fn arrow(&self, column: SortColumn) -> &'static str {
        if self.column != column {
            ""
        } else if self.ascending {
            " ^"
        } else {
            " v"
        }
    }
}

/// What the user asked the bin to do.
///
/// Each variant maps onto work the host performs: [`MediaBinAction::Import`]
/// onto an [`ImportQueue`](crate::media_import::ImportQueue), and every other
/// variant onto exactly one `sub-edit` command — `CreateBin`, `RenameBin`,
/// `MoveBin`, `MoveToBin` and `RelinkMedia` — so nothing the bin does escapes
/// undo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaBinAction {
    /// Import these files, filing them in this bin.
    Import {
        /// The files chosen in the dialog or dropped from the desktop.
        paths: Vec<PathBuf>,
        /// The bin they are filed in.
        bin: BinId,
    },
    /// Create a folder with this name inside this one.
    CreateBin {
        /// The bin the new one goes inside.
        parent: BinId,
        /// Its name.
        name: String,
    },
    /// Rename this folder.
    RenameBin {
        /// The bin to rename.
        bin: BinId,
        /// Its new name.
        name: String,
    },
    /// Move this folder inside another one.
    MoveBin {
        /// The bin to move.
        bin: BinId,
        /// Its new parent.
        parent: BinId,
    },
    /// File this item in another folder.
    MoveMedia {
        /// The item to file.
        media: MediaId,
        /// The bin it goes in.
        bin: BinId,
    },
    /// Point this offline item at another file.
    ///
    /// The host opens the [`RelinkDialog`](crate::relink_dialog::RelinkDialog)
    /// for it: either the user picks the file, or a folder search finds it by
    /// content hash.
    Relink(MediaId),
    /// Relink every offline item at once.
    ///
    /// The host opens the same dialog for all of them; what it finds is
    /// applied as a single undo step.
    RelinkAll,
}

impl MediaBinAction {
    /// The command this action commits, when it is one command on its own.
    ///
    /// Four of the six are: creating, renaming and reparenting a folder, and
    /// filing an item in one. The other two are not edits by themselves —
    /// importing has to hash and probe the files first, and relinking has to
    /// find the replacement — so they return `None` and the host runs the
    /// import queue or the relink dialog, which then commits a command of its
    /// own.
    #[must_use]
    pub fn into_command(self) -> Option<BoxedCommand> {
        match self {
            Self::CreateBin { parent, name } => Some(Box::new(CreateBin::new(name).inside(parent))),
            Self::RenameBin { bin, name } => Some(Box::new(RenameBin::new(bin, name))),
            Self::MoveBin { bin, parent } => Some(Box::new(MoveBin::new(bin, parent))),
            Self::MoveMedia { media, bin } => Some(Box::new(MoveToBin::new(media, bin))),
            Self::Import { .. } | Self::Relink(_) | Self::RelinkAll => None,
        }
    }
}

/// The item an editor is dragging out of the bin.
///
/// This is the payload of an egui drag-and-drop, so it crosses panels: the bin
/// starts the drag and the timeline reads it while the pointer is over a lane
/// and takes it when the button comes up. It carries the item's identity and
/// nothing else — where the clip lands, how long it is and whether the track
/// will have it are the timeline's business
/// ([`plan_source_edit`](crate::source_edit::plan_source_edit)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BinDrag {
    /// The item being dragged.
    pub media: MediaId,
}

/// The widget id the row or tile of `media` drags with.
///
/// Derived from the item's identity, so the same item is the same drag source
/// in the list and the grid, and a test can name it without painting first.
#[must_use]
pub fn drag_source_id(media: MediaId) -> egui::Id {
    egui::Id::new(("media-bin-drag", media))
}

/// The item being dragged out of the bin right now, if one is.
///
/// The timeline asks this while it paints, so a drop target can be drawn
/// before the button comes up.
#[must_use]
pub fn dragged_media(ctx: &egui::Context) -> Option<MediaId> {
    egui::DragAndDrop::payload::<BinDrag>(ctx).map(|drag| drag.media)
}

/// What the bin has selected: a folder, or an item inside one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinSelection {
    /// A folder is selected; its contents are on the right.
    Bin(BinId),
    /// An item inside the open folder is selected.
    Media(MediaId),
}

/// The media bin panel.
#[derive(Debug, Clone, Default)]
pub struct MediaBinPanel {
    /// Whether the contents are listed or tiled.
    pub mode: BinViewMode,
    /// Which column the contents are ordered by.
    pub sort: BinSort,
    /// The open folder. The root bin until one is clicked.
    open_bin: Option<BinId>,
    /// The selected item, when one is selected.
    selected_media: Option<MediaId>,
    /// The folders whose children are hidden.
    collapsed: BTreeSet<BinId>,
    /// The name being typed for a new folder.
    new_bin_name: String,
    /// The name being typed for the open folder.
    rename_buffer: String,
}

impl MediaBinPanel {
    /// A panel showing the root bin, listed and sorted by name.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The folder whose contents are on the right, which is the root bin until
    /// another is opened or after the open one is deleted.
    #[must_use]
    pub fn open_bin(&self, project: &Project) -> BinId {
        self.open_bin
            .filter(|id| project.root_bin.find(*id).is_some())
            .unwrap_or(project.root_bin.id)
    }

    /// Opens `bin` and clears the item selection.
    pub fn open(&mut self, bin: BinId) {
        self.open_bin = Some(bin);
        self.selected_media = None;
    }

    /// Selects `media` inside the open folder.
    pub fn select_media(&mut self, media: MediaId) {
        self.selected_media = Some(media);
    }

    /// What is selected: the item when one is, otherwise the open folder.
    #[must_use]
    pub fn selection(&self, project: &Project) -> BinSelection {
        self.selected_media.map_or_else(
            || BinSelection::Bin(self.open_bin(project)),
            BinSelection::Media,
        )
    }

    /// Whether `bin`'s children are hidden.
    #[must_use]
    pub fn is_collapsed(&self, bin: BinId) -> bool {
        self.collapsed.contains(&bin)
    }

    /// Hides or shows `bin`'s children.
    pub fn toggle_collapsed(&mut self, bin: BinId) {
        if !self.collapsed.remove(&bin) {
            self.collapsed.insert(bin);
        }
    }

    /// The tree rows on screen: every bin whose ancestors are all expanded,
    /// parents before children, each with its depth.
    #[must_use]
    pub fn rows<'a>(&self, project: &'a Project) -> Vec<(usize, &'a Bin)> {
        let mut rows = Vec::new();
        self.push_rows(&project.root_bin, 0, &mut rows);
        rows
    }

    fn push_rows<'a>(&self, bin: &'a Bin, depth: usize, rows: &mut Vec<(usize, &'a Bin)>) {
        rows.push((depth, bin));
        if self.is_collapsed(bin.id) {
            return;
        }
        for child in &bin.children {
            self.push_rows(child, depth + 1, rows);
        }
    }

    /// Draws the panel and returns what the user asked for.
    pub fn ui(&mut self, ui: &mut Ui, project: &Project) -> Vec<MediaBinAction> {
        let mut actions = Vec::new();
        let open = self.open_bin(project);

        // A drop from the desktop is an import into the open folder. It is
        // read before anything else so a drop that lands on a control still
        // imports.
        let dropped = dropped_paths(ui.ctx());
        if !dropped.is_empty() {
            actions.push(MediaBinAction::Import {
                paths: dropped,
                bin: open,
            });
        }

        self.toolbar(ui, project, open, &mut actions);
        ui.separator();
        ui.horizontal_top(|ui| {
            ui.allocate_ui(Vec2::new(TREE_WIDTH, ui.available_height()), |ui| {
                self.tree(ui, project, open, &mut actions);
            });
            ui.separator();
            ui.vertical(|ui| match self.mode {
                BinViewMode::List => self.list(ui, project, open, &mut actions),
                BinViewMode::Grid => self.grid(ui, project, open, &mut actions),
            });
        });
        actions
    }

    /// Import, the view toggle, and the folder controls for the open folder.
    fn toolbar(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        open: BinId,
        actions: &mut Vec<MediaBinAction>,
    ) {
        ui.horizontal(|ui| {
            if ui.button("Import...").clicked() {
                let paths = pick_media_files();
                if !paths.is_empty() {
                    actions.push(MediaBinAction::Import { paths, bin: open });
                }
            }
            if ui.button(self.mode.other().label()).clicked() {
                self.mode = self.mode.other();
            }
            ui.separator();

            ui.add(
                egui::TextEdit::singleline(&mut self.new_bin_name)
                    .desired_width(110.0)
                    .hint_text("New folder"),
            );
            if ui.button("New folder").clicked() {
                let name = folder_name(&self.new_bin_name);
                self.new_bin_name.clear();
                actions.push(MediaBinAction::CreateBin { parent: open, name });
            }

            ui.add(
                egui::TextEdit::singleline(&mut self.rename_buffer)
                    .desired_width(110.0)
                    .hint_text("Rename to"),
            );
            let named = !self.rename_buffer.trim().is_empty();
            if ui.add_enabled(named, egui::Button::new("Rename")).clicked() {
                let name = folder_name(&self.rename_buffer);
                self.rename_buffer.clear();
                actions.push(MediaBinAction::RenameBin { bin: open, name });
            }
        });
        ui.horizontal(|ui| {
            let path = bin_path(project, open);
            ui.label(RichText::new(path).weak());

            // Bulk relink is only worth offering when something is offline,
            // and it says how much is, because that is the number the one
            // undo step will cover.
            let offline = project.offline_media().count();
            if offline > 0 {
                ui.separator();
                ui.label(
                    RichText::new(format!("{offline} offline"))
                        .small()
                        .color(Color32::from_rgb(220, 120, 90)),
                );
                if ui.small_button("Relink all").clicked() {
                    actions.push(MediaBinAction::RelinkAll);
                }
            }
        });
    }

    /// The folder tree: one row per visible bin, each a target for whatever is
    /// selected.
    fn tree(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        open: BinId,
        actions: &mut Vec<MediaBinAction>,
    ) {
        let selection = self.selection(project);
        let rows = self
            .rows(project)
            .into_iter()
            .map(|(depth, bin)| (depth, bin.id, bin.name.clone(), bin.children.is_empty()))
            .collect::<Vec<_>>();
        for (depth, id, name, leaf) in rows {
            ui.horizontal(|ui| {
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "a nesting depth deep enough to lose precision cannot be drawn"
                )]
                ui.add_space(depth as f32 * INDENT);
                if leaf {
                    ui.add_space(INDENT);
                } else {
                    let mark = if self.is_collapsed(id) { "+" } else { "-" };
                    if ui.small_button(mark).clicked() {
                        self.toggle_collapsed(id);
                    }
                }
                if ui.selectable_label(id == open, &name).clicked() {
                    self.open(id);
                }
                if let Some(action) = move_here(selection, id, project)
                    && ui.small_button("Move here").on_hover_text(&name).clicked()
                {
                    actions.push(action);
                }
            });
        }
    }

    /// The list view: sortable headings, then one row per item.
    fn list(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        open: BinId,
        actions: &mut Vec<MediaBinAction>,
    ) {
        ui.horizontal(|ui| {
            for column in SortColumn::ALL {
                let heading = format!("{}{}", column.label(), self.sort.arrow(column));
                let button = egui::Button::new(RichText::new(heading).strong())
                    .min_size(Vec2::new(column.width(), 0.0));
                if ui.add(button).clicked() {
                    self.sort.click(column);
                }
            }
        });
        ui.separator();

        let items = self.contents(project, open);
        if items.is_empty() {
            ui.label(RichText::new("This folder is empty.").weak());
            return;
        }
        for item in items {
            ui.horizontal(|ui| {
                for column in SortColumn::ALL {
                    let text = RichText::new(column.text(item));
                    let text = if item.offline && column == SortColumn::Name {
                        text.color(Color32::from_rgb(220, 120, 90))
                    } else {
                        text
                    };
                    let selected = self.selected_media == Some(item.id);
                    let label = egui::Button::selectable(selected, text);
                    let size = Vec2::new(column.width(), 18.0);
                    // The name is where the item is picked up: an editor
                    // drags a clip onto the timeline by its name, and the
                    // metadata columns stay ordinary buttons.
                    let clicked = if column == SortColumn::Name {
                        ui.dnd_drag_source(
                            drag_source_id(item.id),
                            BinDrag { media: item.id },
                            |ui| ui.add_sized(size, label).clicked(),
                        )
                        .inner
                    } else {
                        ui.add_sized(size, label).clicked()
                    };
                    if clicked {
                        self.select_media(item.id);
                    }
                }
                Self::proxy_badge(ui, item);
                self.offline_controls(ui, item, actions);
            });
        }
    }

    /// The grid view: one tile per item.
    ///
    /// The tile draws the item's name and metadata over a placeholder frame.
    /// The strip its picture comes from is generated at import
    /// (`sub_media::spawn_thumbnail_job`); painting that JPEG needs an image
    /// decoder the UI does not carry yet.
    fn grid(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        open: BinId,
        actions: &mut Vec<MediaBinAction>,
    ) {
        let items = self.contents(project, open);
        if items.is_empty() {
            ui.label(RichText::new("This folder is empty.").weak());
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for item in items {
                let selected = self.selected_media == Some(item.id);
                ui.allocate_ui(TILE, |ui| {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        let name = RichText::new(&item.name).strong();
                        let clicked = ui
                            .dnd_drag_source(
                                drag_source_id(item.id),
                                BinDrag { media: item.id },
                                |ui| ui.add(egui::Button::selectable(selected, name)).clicked(),
                            )
                            .inner;
                        if clicked {
                            self.select_media(item.id);
                        }
                        ui.label(RichText::new(duration_text(item)).weak());
                        ui.label(RichText::new(resolution_text(item)).weak());
                        Self::proxy_badge(ui, item);
                        self.offline_controls(ui, item, actions);
                    });
                });
            }
        });
    }

    /// The proxy badge: what stands in for this item while editing.
    ///
    /// Every state says its name, an item with no proxy included, so the bin
    /// answers "is this cut running on proxies?" at a glance rather than only
    /// when something is wrong. The badge is a label, not a control: asking
    /// for a proxy is a job the host starts, and what it reports comes back as
    /// a [`SetProxyState`](sub_edit::commands::SetProxyState) command.
    fn proxy_badge(ui: &mut Ui, item: &MediaItem) {
        let badge = RichText::new(proxy_text(item))
            .small()
            .color(proxy_color(item));
        let response = ui.label(badge);
        if let Some(hover) = proxy_hover(item) {
            response.on_hover_text(hover);
        }
    }

    /// The offline badge and the relink button, for an item whose file is
    /// missing.
    fn offline_controls(
        &mut self,
        ui: &mut Ui,
        item: &MediaItem,
        actions: &mut Vec<MediaBinAction>,
    ) {
        if !item.offline {
            return;
        }
        ui.label(
            RichText::new("OFFLINE")
                .small()
                .color(Color32::from_rgb(220, 120, 90)),
        );
        if ui.small_button("Relink").clicked() {
            self.select_media(item.id);
            actions.push(MediaBinAction::Relink(item.id));
        }
    }

    /// The items filed in `bin`, in the order the sort puts them.
    fn contents<'a>(&self, project: &'a Project, bin: BinId) -> Vec<&'a MediaItem> {
        project
            .root_bin
            .find(bin)
            .map(|bin| sorted_media(project, bin, self.sort))
            .unwrap_or_default()
    }
}

/// The action that moves `selection` into `target`, when that is a move worth
/// offering.
///
/// A bin cannot be moved into itself, into its own subtree or into the parent
/// it already has, and an item cannot be filed in the bin it is already in;
/// the button is absent in each of those cases rather than raising a command
/// the model would refuse.
fn move_here(selection: BinSelection, target: BinId, project: &Project) -> Option<MediaBinAction> {
    match selection {
        BinSelection::Bin(bin) => {
            // The root bin is the tree itself and has no parent to leave.
            if bin == project.root_bin.id {
                return None;
            }
            // Moving a bin into itself or into its own subtree would detach
            // that subtree; `MoveBin` refuses both, so the button is absent
            // rather than offering a command the model would reject.
            let moving = project.root_bin.find(bin)?;
            if moving.find(target).is_some() {
                return None;
            }
            // Already there.
            let parent = project
                .root_bin
                .iter()
                .find(|candidate| candidate.children.iter().any(|child| child.id == bin))?;
            if parent.id == target {
                return None;
            }
            Some(MediaBinAction::MoveBin {
                bin,
                parent: target,
            })
        }
        BinSelection::Media(media) => {
            if project.bin_of(media) == Some(target) {
                return None;
            }
            Some(MediaBinAction::MoveMedia { media, bin: target })
        }
    }
}

/// The items filed directly in `bin`, ordered by `sort`.
///
/// Items the project no longer holds are skipped, so a bin listing a stale
/// identifier draws rather than panicking.
#[must_use]
pub fn sorted_media<'a>(project: &'a Project, bin: &Bin, sort: BinSort) -> Vec<&'a MediaItem> {
    let mut items: Vec<&MediaItem> = bin
        .media
        .iter()
        .filter_map(|id| project.media_item(*id))
        .collect();
    items.sort_by(|left, right| {
        let ordering = match sort.column {
            SortColumn::Name => compare_names(left, right),
            SortColumn::Duration => duration_of(left).cmp(&duration_of(right)),
            SortColumn::FrameRate => compare_rates(frame_rate_of(left), frame_rate_of(right)),
            SortColumn::Resolution => pixels_of(left).cmp(&pixels_of(right)),
        }
        // Ties keep a stable, predictable order rather than the order the bin
        // happens to list them in.
        .then_with(|| compare_names(left, right));
        if sort.ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    items
}

/// Names compare case-insensitively, then exactly, so `a.mov` and `A.mov` have
/// a stable order.
fn compare_names(left: &MediaItem, right: &MediaItem) -> std::cmp::Ordering {
    left.name
        .to_lowercase()
        .cmp(&right.name.to_lowercase())
        .then_with(|| left.name.cmp(&right.name))
}

/// The item's duration, or zero at one unit per second when it has none.
///
/// [`RationalTime`] compares wall-clock lengths exactly, so a 24 fps item and
/// a 23.976 fps one sort against each other correctly.
fn duration_of(item: &MediaItem) -> RationalTime {
    item.info
        .as_ref()
        .and_then(|info| info.duration)
        .unwrap_or_else(|| RationalTime::zero(Rational::ONE))
}

/// The frame rate of the item's first video stream.
fn frame_rate_of(item: &MediaItem) -> Option<Rational> {
    item.info
        .as_ref()
        .and_then(|info| info.video.first())
        .map(|stream| stream.frame_rate)
}

/// The pixel count and width of the item's first video stream, which is what
/// the resolution column orders by. Audio-only items sort first.
fn pixels_of(item: &MediaItem) -> (u64, u32) {
    item.info
        .as_ref()
        .and_then(|info| info.video.first())
        .map_or((0, 0), |stream| {
            (
                u64::from(stream.width) * u64::from(stream.height),
                stream.width,
            )
        })
}

/// Compares two rates exactly, by cross-multiplying in `u64`.
///
/// The derived ordering on [`Rational`] compares the numerator first, which
/// would put 24 fps below 23.976 fps; this does not.
fn compare_rates(left: Option<Rational>, right: Option<Rational>) -> std::cmp::Ordering {
    match (left, right) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(left), Some(right)) => {
            let lhs = u64::from(left.numerator()) * u64::from(right.denominator());
            let rhs = u64::from(right.numerator()) * u64::from(left.denominator());
            lhs.cmp(&rhs)
        }
    }
}

/// The duration column: a timecode at the item's own rate.
///
/// Falls back to a frame count when the rate is not one timecode can be
/// counted at, and to [`UNKNOWN`] when the file has not been probed.
#[must_use]
pub fn duration_text(item: &MediaItem) -> String {
    let Some(duration) = item.info.as_ref().and_then(|info| info.duration) else {
        return UNKNOWN.to_owned();
    };
    let rate = frame_rate_of(item).unwrap_or_else(|| duration.rate());
    let frames = duration.rescaled_to(rate).value();
    TimecodeRate::new(rate, TimecodeRate::rate_drops_frames(rate)).map_or_else(
        |_| format!("{frames} frames"),
        |rate| Timecode::from_frame_number(frames, rate).to_string(),
    )
}

/// The frame-rate column: the exact rational as a decimal, to three places.
///
/// The arithmetic is integer throughout — 24000/1001 becomes `23.976` by
/// dividing 24000000 by 1001 and rounding to nearest — so no rate is ever a
/// float, and a whole rate reads as `24` rather than `24.000`.
#[must_use]
pub fn frame_rate_text(item: &MediaItem) -> String {
    let Some(rate) = frame_rate_of(item) else {
        return UNKNOWN.to_owned();
    };
    let numerator = u64::from(rate.numerator());
    let denominator = u64::from(rate.denominator());
    if numerator % denominator == 0 {
        return (numerator / denominator).to_string();
    }
    let thousandths = (numerator * 1000 + denominator / 2) / denominator;
    let whole = thousandths / 1000;
    let fraction = format!("{:03}", thousandths % 1000);
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

/// The resolution column: the coded size of the first video stream.
#[must_use]
pub fn resolution_text(item: &MediaItem) -> String {
    item.info
        .as_ref()
        .and_then(|info| info.video.first())
        .map_or_else(
            || {
                if item.info.as_ref().is_some_and(StreamInfo::has_audio) {
                    "audio".to_owned()
                } else {
                    UNKNOWN.to_owned()
                }
            },
            |stream| format!("{}x{}", stream.width, stream.height),
        )
}

/// The proxy badge's text: which of the five proxy states the item is in.
///
/// The wording is the state, not the file: an editor asks whether this clip is
/// running on a proxy, not where the proxy lives.
#[must_use]
pub fn proxy_text(item: &MediaItem) -> String {
    match item.proxy {
        ProxyState::None => "NO PROXY",
        ProxyState::Pending => "PROXY...",
        ProxyState::Ready(_) => "PROXY",
        ProxyState::Stale(_) => "PROXY STALE",
        ProxyState::Failed(_) => "PROXY FAILED",
    }
    .to_owned()
}

/// The badge colour: quiet for the two states nothing is wrong with, and the
/// same warning colour the offline badge uses for the two that need the user.
fn proxy_color(item: &MediaItem) -> Color32 {
    match item.proxy {
        ProxyState::Ready(_) => Color32::from_rgb(120, 190, 130),
        ProxyState::Stale(_) | ProxyState::Failed(_) => Color32::from_rgb(220, 120, 90),
        ProxyState::None | ProxyState::Pending => Color32::GRAY,
    }
}

/// What the badge says on hover, when there is more to say than the state.
fn proxy_hover(item: &MediaItem) -> Option<String> {
    match &item.proxy {
        ProxyState::Ready(path) => Some(format!("Preview uses {}", path.as_str())),
        ProxyState::Stale(_) => Some("The source changed since this proxy was made".to_owned()),
        ProxyState::Failed(message) => Some(message.clone()),
        ProxyState::None | ProxyState::Pending => None,
    }
}

/// The path of `bin` from the root, for the breadcrumb line.
#[must_use]
pub fn bin_path(project: &Project, bin: BinId) -> String {
    fn walk(current: &Bin, target: BinId, trail: &mut Vec<String>) -> bool {
        trail.push(current.name.clone());
        if current.id == target {
            return true;
        }
        for child in &current.children {
            if walk(child, target, trail) {
                return true;
            }
        }
        trail.pop();
        false
    }

    let mut trail = Vec::new();
    if walk(&project.root_bin, bin, &mut trail) {
        trail.join(" / ")
    } else {
        project.root_bin.name.clone()
    }
}

/// The name a typed folder name becomes: trimmed, and never empty.
#[must_use]
pub fn folder_name(typed: &str) -> String {
    let trimmed = typed.trim();
    if trimmed.is_empty() {
        "New folder".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The files dropped onto the window this frame.
///
/// egui hands the integration's file handles over; only their paths matter
/// here, because import reads the file itself on a worker thread.
#[must_use]
pub fn dropped_paths(ctx: &egui::Context) -> Vec<PathBuf> {
    ctx.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_path_buf())
            .collect()
    })
}

/// Asks the operating system for media files to import.
///
/// This is the one blocking call in the panel: a modal file dialog is what the
/// user asked for. It returns an empty list when the dialog is cancelled, and
/// on a machine with no desktop portal at all.
#[must_use]
pub fn pick_media_files() -> Vec<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Import media")
        .add_filter(
            "Media",
            &[
                "mp4", "mov", "mkv", "webm", "avi", "mxf", "m4v", "wav", "flac", "mp3", "aac",
                "m4a", "ogg",
            ],
        )
        .add_filter("All files", &["*"])
        .pick_files()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        BinSort, BinViewMode, MediaBinPanel, SortColumn, bin_path, duration_text, folder_name,
        frame_rate_text, resolution_text, sorted_media,
    };
    use sub_model::media::{AudioStream, StreamInfo, VideoStream};
    use sub_model::{Bin, ColorTags, MediaItem, MediaPath, Project};
    use sub_time::{Rational, RationalTime};

    /// A media item named `name`, `frames` long at `rate` and `width` wide.
    fn item(name: &str, frames: i64, rate: Rational, width: u32, height: u32) -> MediaItem {
        let mut media =
            MediaItem::new(MediaPath::new(format!("footage/{name}")).expect("valid path"));
        name.clone_into(&mut media.name);
        media.info = Some(StreamInfo {
            duration: Some(RationalTime::new(frames, rate)),
            video: vec![VideoStream {
                width,
                height,
                frame_rate: rate,
                sample_aspect: Rational::ONE,
                color: ColorTags::REC709,
            }],
            audio: Vec::new(),
        });
        media
    }

    /// A project whose root bin holds three items of different shapes.
    fn project() -> Project {
        let mut project = Project::new("Doc cut");
        for media in [
            item("c.mov", 48, Rational::FPS_24, 1920, 1080),
            item("a.mov", 100, Rational::FPS_25, 3840, 2160),
            item("b.mov", 24, Rational::FPS_23_976, 1280, 720),
        ] {
            project.root_bin.media.push(media.id);
            project.media.push(media);
        }
        project
    }

    fn names(project: &Project, sort: BinSort) -> Vec<String> {
        sorted_media(project, &project.root_bin, sort)
            .into_iter()
            .map(|item| item.name.clone())
            .collect()
    }

    #[test]
    fn the_name_column_sorts_both_ways() {
        let project = project();
        assert_eq!(
            names(&project, BinSort::by(SortColumn::Name)),
            ["a.mov", "b.mov", "c.mov"]
        );
        let mut sort = BinSort::by(SortColumn::Name);
        sort.click(SortColumn::Name);
        assert!(!sort.ascending);
        assert_eq!(names(&project, sort), ["c.mov", "b.mov", "a.mov"]);
    }

    #[test]
    fn duration_sorts_by_wall_clock_length_not_by_frame_count() {
        let project = project();
        // 24 frames at 23.976 is just over a second; 48 at 24 is two seconds;
        // 100 at 25 is four.
        assert_eq!(
            names(&project, BinSort::by(SortColumn::Duration)),
            ["b.mov", "c.mov", "a.mov"]
        );
    }

    #[test]
    fn frame_rate_sorts_exactly_so_23_976_is_below_24() {
        let project = project();
        assert_eq!(
            names(&project, BinSort::by(SortColumn::FrameRate)),
            ["b.mov", "c.mov", "a.mov"]
        );
    }

    #[test]
    fn resolution_sorts_by_pixel_count() {
        let project = project();
        assert_eq!(
            names(&project, BinSort::by(SortColumn::Resolution)),
            ["b.mov", "c.mov", "a.mov"]
        );
    }

    #[test]
    fn a_column_click_reverses_its_own_column_and_resets_another() {
        let mut sort = BinSort::by(SortColumn::Name);
        sort.click(SortColumn::Duration);
        assert_eq!(sort.column, SortColumn::Duration);
        assert!(sort.ascending);
        sort.click(SortColumn::Duration);
        assert!(!sort.ascending);
        sort.click(SortColumn::Name);
        assert_eq!(sort.column, SortColumn::Name);
        assert!(sort.ascending);
        assert_eq!(sort.arrow(SortColumn::Name), " ^");
        assert_eq!(sort.arrow(SortColumn::Duration), "");
    }

    #[test]
    fn metadata_columns_read_as_timecode_and_exact_rates() {
        let clip = item("take.mov", 48, Rational::FPS_24, 1920, 1080);
        assert_eq!(duration_text(&clip), "00:00:02:00");
        assert_eq!(frame_rate_text(&clip), "24");
        assert_eq!(resolution_text(&clip), "1920x1080");

        let ntsc = item("ntsc.mov", 30, Rational::FPS_29_97, 1920, 1080);
        assert_eq!(frame_rate_text(&ntsc), "29.97");
        let film = item("film.mov", 24, Rational::FPS_23_976, 1920, 1080);
        assert_eq!(frame_rate_text(&film), "23.976");
    }

    #[test]
    fn an_unprobed_item_says_so_in_every_column() {
        let bare = MediaItem::new(MediaPath::new("footage/unknown.mov").expect("valid path"));
        assert_eq!(duration_text(&bare), super::UNKNOWN);
        assert_eq!(frame_rate_text(&bare), super::UNKNOWN);
        assert_eq!(resolution_text(&bare), super::UNKNOWN);

        let mut sound = MediaItem::new(MediaPath::new("audio/vo.wav").expect("valid path"));
        sound.info = Some(StreamInfo {
            duration: Some(RationalTime::new(48_000, Rational::ONE)),
            video: Vec::new(),
            audio: vec![AudioStream {
                channels: 2,
                sample_rate: 48_000,
            }],
        });
        assert_eq!(resolution_text(&sound), "audio");
    }

    #[test]
    fn the_tree_hides_the_children_of_a_collapsed_folder() {
        let mut project = Project::new("Doc cut");
        let mut interviews = Bin::new("Interviews");
        let interviews_id = interviews.id;
        let takes = Bin::new("Takes");
        let takes_id = takes.id;
        interviews.children.push(takes);
        project.root_bin.children.push(interviews);

        let mut panel = MediaBinPanel::new();
        let visible: Vec<_> = panel.rows(&project).iter().map(|(_, bin)| bin.id).collect();
        assert_eq!(
            visible,
            [project.root_bin.id, interviews_id, takes_id],
            "everything is expanded to begin with"
        );
        assert_eq!(panel.rows(&project)[2].0, 2, "depth is the nesting level");

        panel.toggle_collapsed(interviews_id);
        let visible: Vec<_> = panel.rows(&project).iter().map(|(_, bin)| bin.id).collect();
        assert_eq!(visible, [project.root_bin.id, interviews_id]);
        panel.toggle_collapsed(interviews_id);
        assert_eq!(panel.rows(&project).len(), 3);
    }

    #[test]
    fn the_open_folder_falls_back_to_the_root_when_it_is_gone() {
        let mut project = Project::new("Doc cut");
        let bin = Bin::new("Interviews");
        let bin_id = bin.id;
        project.root_bin.children.push(bin);

        let mut panel = MediaBinPanel::new();
        assert_eq!(panel.open_bin(&project), project.root_bin.id);
        panel.open(bin_id);
        assert_eq!(panel.open_bin(&project), bin_id);
        assert_eq!(bin_path(&project, bin_id), "Doc cut / Interviews");

        project.root_bin.children.clear();
        assert_eq!(panel.open_bin(&project), project.root_bin.id);
    }

    #[test]
    fn a_typed_folder_name_is_trimmed_and_never_empty() {
        assert_eq!(folder_name("  Interviews "), "Interviews");
        assert_eq!(folder_name("   "), "New folder");
    }

    #[test]
    fn the_view_toggle_names_the_mode_it_switches_to() {
        assert_eq!(BinViewMode::List.other(), BinViewMode::Grid);
        assert_eq!(BinViewMode::Grid.other(), BinViewMode::List);
        assert_eq!(BinViewMode::Grid.label(), "Grid");
    }
}
