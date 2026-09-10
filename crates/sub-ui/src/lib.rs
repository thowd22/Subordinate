//! The egui application: timeline, media bin, viewer, inspector and export panels.
//!
//! The UI thread never blocks on media. The viewer can pop out into its own
//! viewport for a second display. See docs/PLAN.md §5.7.
//!
//! eframe is configured for its wgpu backend, so the device egui paints with
//! is the same device `sub-render` composites with; see [`app`].

pub mod app;
pub mod audio_settings;
pub mod diagnostics;
pub mod dock;
pub mod history_panel;
pub mod keymap;
pub mod media_bin;
pub mod media_import;
pub mod meter;
pub mod plugins;
pub mod popout;
pub mod relink_dialog;
pub mod sequence_tabs;
pub mod shortcuts;
pub mod snapping;
pub mod thumbnails;
pub mod timeline;
pub mod timeline_panel;
pub mod track_header;
pub mod viewer;
pub mod waveform;

pub use app::{AppOptions, SubordinateApp, run};
pub use audio_settings::{AudioSettingsAction, AudioSettingsPanel, device_label, status_line};
pub use diagnostics::DiagnosticsPanel;
pub use dock::{DockLayout, LAYOUT_FILE_NAME, LAYOUT_VERSION, LoadedLayout, Panel, layout_menu_ui};
pub use history_panel::{
    HistoryAction, HistoryList, HistoryPanel, ORIGINAL_STATE_LABEL, edit_menu_ui,
};
pub use keymap::{LoadedKeymap, chord_spec, config_dir, keymap_path, parse_chord};
pub use media_bin::{
    BinSelection, BinSort, BinViewMode, MediaBinAction, MediaBinPanel, SortColumn, bin_path,
    dropped_paths, duration_text, folder_name, frame_rate_text, pick_media_files, resolution_text,
    sorted_media,
};
pub use media_import::{
    IMPORT_JOB_KIND, ImportJob, ImportOptions, ImportOutcome, ImportQueue, imported_item,
    spawn_import_job, stream_info,
};
pub use meter::{
    CLIP_COLOR, CLIP_HOLD_SECONDS, MIN_DB, MeterState, NORMAL_COLOR, PEAK_FALL_DB_PER_SECOND,
    PEAK_HOLD_SECONDS, WARN_COLOR, amplitude_fraction, amplitude_to_db, db_fraction,
};
pub use plugins::{EMPTY_LABEL, MENU_TITLE, PluginMenu, PluginMenuEntry, plugins_menu_ui};
pub use popout::{
    CLOSE_LABEL, OPEN_LABEL, POPOUT_TITLE, PopoutShared, PopoutViewer, is_playback_action,
    playback_shortcuts, popout_content_ui, popout_menu_ui, popout_viewport_id, popout_viewport_ui,
};
pub use relink_dialog::{
    RELINK_JOB_KIND, RelinkDialog, is_certain, pick_replacement_file, pick_search_folder,
};
pub use sequence_tabs::{
    NewSequenceDialog, SequenceTabAction, SequenceTabs, SequenceViewState, default_sequence_name,
};
pub use shortcuts::{
    Action, Binding, Category, Conflict, DEFAULT_BINDINGS, HelpRow, ShortcutMap, ShortcutsWindow,
    help_rows,
};
pub use snapping::{
    DEFAULT_THRESHOLD_PX, SnapCandidate, SnapKind, SnapSettings, collect_candidates, snap,
    snapped_time, track_edges,
};
pub use thumbnails::{
    BUCKET_SIZES, DEFAULT_BUDGET_BYTES, DEFAULT_UPLOADS_PER_FRAME, ThumbnailCache,
    ThumbnailCacheConfig, ThumbnailCacheStats, ZoomBucket, fitted_size, scale_to_bucket, tile_time,
};
pub use timeline::{ClipPlacement, TimelineView, TrackLayout, ZoomLevel};
pub use timeline_panel::{
    ClipMediaKind, PanelLayout, StripTiles, TimelineMetrics, TimelinePanel, TimelineResponse,
    TrimmedEdges, WheelInput, clip_edits_allowed, strip_tiles,
};
pub use track_header::{HeaderLayout, MenuChoice, MenuEntry, TrackAction, TrackHeaderState};
pub use viewer::{
    POPPED_OUT_LABEL, TransportAction, ViewerAction, ViewerFit, ViewerFrame, ViewerPanel,
    ViewerState, paint_picture,
};
pub use waveform::{
    CHANNEL_TEXELS, ClipWaveform, MAX_TEXTURE_TEXELS, WaveformCache, frames_per_pixel,
    level_for_zoom, waveform_image,
};

/// The stable error codes this crate reports.
pub mod codes {
    use sub_core::ErrorCode;

    /// A zoom level falls outside the timeline's zoom ladder.
    pub const INVALID_ZOOM: ErrorCode = ErrorCode::from_static("ui.invalid_zoom");

    /// A sequence id does not name a sequence in this project.
    pub const UNKNOWN_SEQUENCE: ErrorCode = ErrorCode::from_static("ui.unknown_sequence");

    /// The only sequence left cannot be deleted.
    pub const LAST_SEQUENCE: ErrorCode = ErrorCode::from_static("ui.last_sequence");

    /// One keyboard chord is claimed by more than one action.
    pub const SHORTCUT_CONFLICT: ErrorCode = ErrorCode::from_static("ui.shortcut_conflict");

    /// A keymap file is not valid TOML, or an entry has the wrong shape.
    pub const KEYMAP_PARSE: ErrorCode = ErrorCode::from_static("ui.keymap_parse");

    /// A keymap entry names an action this editor does not have.
    pub const KEYMAP_UNKNOWN_ACTION: ErrorCode = ErrorCode::from_static("ui.keymap_unknown_action");

    /// A keymap entry's value is not a keyboard chord.
    pub const KEYMAP_INVALID_CHORD: ErrorCode = ErrorCode::from_static("ui.keymap_invalid_chord");

    /// A keymap file exists but could not be read.
    pub const KEYMAP_UNREADABLE: ErrorCode = ErrorCode::from_static("ui.keymap_unreadable");

    /// A plugin asked for a shortcut that is not a keyboard chord.
    pub const PLUGIN_INVALID_CHORD: ErrorCode = ErrorCode::from_static("ui.plugin_invalid_chord");

    /// A `layout.json` is not valid JSON, names a panel this build does not
    /// have, or was written by a different schema version.
    pub const LAYOUT_PARSE: ErrorCode = ErrorCode::from_static("ui.layout_parse");

    /// A `layout.json` exists but could not be read.
    pub const LAYOUT_UNREADABLE: ErrorCode = ErrorCode::from_static("ui.layout_unreadable");

    /// The panel layout could not be written to the config directory.
    pub const LAYOUT_UNWRITABLE: ErrorCode = ErrorCode::from_static("ui.layout_unwritable");

    /// A plugin asked for a chord the editor or an earlier plugin already
    /// holds. The command keeps its menu entry and loses its shortcut.
    pub const PLUGIN_SHORTCUT_CONFLICT: ErrorCode =
        ErrorCode::from_static("ui.plugin_shortcut_conflict");
}
