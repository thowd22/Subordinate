//! The egui application: timeline, media bin, viewer, inspector and export panels.
//!
//! The UI thread never blocks on media. The viewer can pop out into its own
//! viewport for a second display. See docs/PLAN.md §5.7.
//!
//! eframe is configured for its wgpu backend, so the device egui paints with
//! is the same device `sub-render` composites with; see [`app`].

pub mod app;
pub mod diagnostics;
pub mod media_bin;
pub mod media_import;
pub mod sequence_tabs;
pub mod shortcuts;
pub mod thumbnails;
pub mod timeline;
pub mod timeline_panel;
pub mod track_header;
pub mod viewer;

pub use app::{AppOptions, SubordinateApp, run};
pub use diagnostics::DiagnosticsPanel;
pub use media_bin::{
    BinSelection, BinSort, BinViewMode, MediaBinAction, MediaBinPanel, SortColumn, bin_path,
    dropped_paths, duration_text, folder_name, frame_rate_text, pick_media_files, resolution_text,
    sorted_media,
};
pub use media_import::{
    IMPORT_JOB_KIND, ImportJob, ImportOptions, ImportOutcome, ImportQueue, imported_item,
    spawn_import_job, stream_info,
};
pub use sequence_tabs::{
    NewSequenceDialog, SequenceTabAction, SequenceTabs, SequenceViewState, default_sequence_name,
};
pub use shortcuts::{
    Action, Binding, Category, Conflict, DEFAULT_BINDINGS, HelpRow, ShortcutMap, ShortcutsWindow,
    help_rows,
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
pub use viewer::{ViewerAction, ViewerFit, ViewerFrame, ViewerPanel, ViewerState};

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
}
