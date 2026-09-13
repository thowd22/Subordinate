//! The egui application: timeline, media bin, viewer, inspector and export panels.
//!
//! The UI thread never blocks on media. The viewer can pop out into its own
//! viewport for a second display. See docs/PLAN.md §5.7.
//!
//! eframe is configured for its wgpu backend, so the device egui paints with
//! is the same device `sub-render` composites with; see [`app`].

pub mod app;
pub mod audio_settings;
pub mod command_api;
pub mod diagnostics;
pub mod dock;
pub mod effects;
pub mod export_panel;
pub mod export_runner;
pub mod fade;
pub mod fullscreen;
pub mod history_panel;
pub mod host_services;
pub mod inspector;
pub mod keymap;
pub mod markers;
pub mod media_bin;
pub mod media_import;
pub mod meter;
pub mod playback_audio;
pub mod plugins;
pub mod plugins_panel;
pub mod popout;
pub mod preview;
pub mod recovery;
pub mod relink_dialog;
pub mod selection;
pub mod sequence_tabs;
pub mod session;
pub mod shortcuts;
pub mod snapping;
pub mod source_edit;
pub mod split;
pub mod thumbnails;
pub mod timeline;
pub mod timeline_panel;
pub mod track_header;
pub mod transition;
pub mod trim;
pub mod viewer;
pub mod waveform;

pub use app::{
    AppOptions, IMPORT_GROUP_LABEL, NO_IMPORT_REASON, ProjectState, SubordinateApp, UI_SMOKE_READY,
    edit_mode_for, run,
};
pub use audio_settings::{AudioSettingsAction, AudioSettingsPanel, device_label, status_line};
pub use command_api::CommandApi;
pub use diagnostics::DiagnosticsPanel;
pub use dock::{DockLayout, LAYOUT_FILE_NAME, LAYOUT_VERSION, LoadedLayout, Panel, layout_menu_ui};
pub use export_panel::{
    AUTOMATIC_ENCODER_LABEL, CANCEL_LABEL, ExportAction, ExportPanel, ExportRange, ExportRequest,
    ExportStatus, NO_PRESETS_LABEL, NO_RECENT_LABEL, OPEN_FOLDER_LABEL, PresetEntry, PresetSource,
    RECENT_LIMIT, RecentExport, START_LABEL, sequence_frames,
};
pub use export_runner::{
    EXPORT_PRIORITY, ExportRunner, ExportSources, ExportStreams, elements_for, frames_total,
    settings_for,
};
pub use fullscreen::{
    FULLSCREEN_FILE_NAME, FULLSCREEN_VERSION, FullscreenAction, FullscreenSettings,
    FullscreenState, LoadedFullscreen, Monitor, MonitorChoice, MonitorList, monitor_picker_ui,
};
pub use history_panel::{
    EARLIER_STEP_LABEL, HistoryAction, HistoryList, HistoryPanel, LATER_STEP_LABEL,
    ORIGINAL_STATE_LABEL, edit_menu_ui,
};
pub use host_services::{GuiHost, GuiServices, QueuedExport};
pub use inspector::{
    InspectorField, InspectorPanel, InspectorResponse, apply_edit as apply_inspector_edit,
};
pub use keymap::{LoadedKeymap, chord_spec, config_dir, keymap_path, parse_chord};
pub use markers::{
    DEFAULT_MARKER_NAME, MARKER_PALETTE, MarkerAction, MarkerState, marker_color, moved_range,
};
pub use media_bin::{
    BinDrag, BinSelection, BinSort, BinStatus, BinViewMode, MediaBinAction, MediaBinPanel,
    PENDING_LABEL, SortColumn, bin_path, drag_source_id, dragged_media, dropped_paths,
    duration_text, file_label, folder_name, frame_rate_text, pick_media_files, resolution_text,
    sorted_media,
};
pub use media_import::{
    FinishedImport, IMPORT_JOB_KIND, ImportBatch, ImportJob, ImportOptions, ImportOutcome,
    ImportQueue, imported_item, spawn_import_job, stream_info,
};
pub use meter::{
    CLIP_COLOR, CLIP_HOLD_SECONDS, MIN_DB, MeterState, NORMAL_COLOR, PEAK_FALL_DB_PER_SECOND,
    PEAK_HOLD_SECONDS, WARN_COLOR, amplitude_fraction, amplitude_to_db, db_fraction,
};
pub use plugins::{EMPTY_LABEL, MENU_TITLE, PluginMenu, PluginMenuEntry, plugins_menu_ui};
pub use plugins_panel::{
    LoadStatus, PluginAction, PluginOutcome, PluginRow, PluginsPanel, open_folder,
};
pub use popout::{
    CLOSE_LABEL, OPEN_LABEL, POPOUT_TITLE, PopoutPlacement, PopoutShared, PopoutViewer,
    is_playback_action, playback_shortcuts, popout_content_ui, popout_menu_ui,
    popout_viewport_builder, popout_viewport_id, popout_viewport_ui,
};
// `duration_text` and the two label constants keep their module paths: the
// media bin already exports a `duration_text` (a clip's length, not a wall
// clock span) and a `MENU_TITLE`/`EMPTY_LABEL` pair belongs to the plugin
// menu.
pub use fade::{FadeEdge, FadeEdit, FadeRefusal, apply_fade, plan_fade};
pub use preview::{
    DECODE_AHEAD_FRAMES, MAX_OPEN_CLIPS, PREVIEW_JOB_KIND, PreviewFrames, PreviewService,
    PreviewStats,
};
pub use recovery::{
    DISCARD_LABEL, NO_PROJECT_LABEL, PROMPT_TITLE, RECOVER_LABEL, RecoveryOutcome, RecoveryPrompt,
    SnapshotMenu, entry_label,
};
pub use relink_dialog::{
    RELINK_JOB_KIND, RelinkDialog, is_certain, pick_replacement_file, pick_search_folder,
};
pub use selection::{
    ClipRef, MoveGroup, MoveRefusal, PreviewedMove, Selection, apply_move, clips_in_marquee,
    plan_move,
};
pub use sequence_tabs::{
    NewSequenceDialog, SequenceTabAction, SequenceTabs, SequenceViewState, default_sequence_name,
};
pub use session::EditorSession;
pub use shortcuts::{
    Action, Binding, Category, Conflict, DEFAULT_BINDINGS, HelpRow, ShortcutMap, ShortcutsWindow,
    help_rows,
};
pub use snapping::{
    DEFAULT_THRESHOLD_PX, SnapCandidate, SnapKind, SnapSettings, collect_candidates, snap,
    snapped_time, track_edges,
};
pub use source_edit::{EditMode, PlannedEdit, SourceRefusal, apply_source_edit, plan_source_edit};
pub use split::{SplitCut, SplitGroup, SplitRefusal, apply_split, plan_split, plan_split_clip};
pub use thumbnails::{
    BUCKET_SIZES, DEFAULT_BUDGET_BYTES, DEFAULT_UPLOADS_PER_FRAME, ThumbnailCache,
    ThumbnailCacheConfig, ThumbnailCacheStats, ZoomBucket, fitted_size, scale_to_bucket, tile_time,
};
pub use timeline::{ClipPlacement, TimelineView, TrackLayout, ZoomLevel};
pub use timeline_panel::{
    ClipMediaKind, FADE_ALPHA, FADE_BAND_PX, FADE_COLOR, FADE_HANDLE_PX, MARKER_FLAG_HEIGHT,
    MARKER_FLAG_WIDTH, PanelLayout, StripTiles, TRIM_HANDLE_PX, TimelineMetrics, TimelinePanel,
    TimelineResponse, Tool, TrimmedEdges, WheelInput, clip_edits_allowed, strip_tiles,
};
pub use track_header::{
    HeaderLayout, HeaderOutcome, MenuChoice, MenuEntry, TrackAction, TrackHeaderState,
    apply_actions,
};
pub use trim::{TrimEdge, TrimGroup, TrimRefusal, TrimStep, apply_trim, plan_trim};
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

    /// Media was asked to be imported into a project that has never been
    /// saved, so there is no folder for its path to be relative to.
    pub const IMPORT_NOT_READY: ErrorCode = ErrorCode::from_static("ui.import_not_ready");

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

    /// A `fullscreen.json` is not valid JSON, or was written by a different
    /// schema version.
    pub const FULLSCREEN_PARSE: ErrorCode = ErrorCode::from_static("ui.fullscreen_parse");

    /// A `fullscreen.json` exists but could not be read.
    pub const FULLSCREEN_UNREADABLE: ErrorCode = ErrorCode::from_static("ui.fullscreen_unreadable");

    /// The fullscreen display choice could not be written to the config
    /// directory.
    pub const FULLSCREEN_UNWRITABLE: ErrorCode = ErrorCode::from_static("ui.fullscreen_unwritable");

    /// A project file exists but could not be read or parsed.
    pub const PROJECT_UNREADABLE: ErrorCode = ErrorCode::from_static("ui.project_unreadable");

    /// The project could not be written: no file has been chosen for it yet,
    /// or the write itself failed.
    pub const PROJECT_UNSAVED: ErrorCode = ErrorCode::from_static("ui.project_unsaved");

    /// A plugin asked for a chord the editor or an earlier plugin already
    /// holds. The command keeps its menu entry and loses its shortcut.
    pub const PLUGIN_SHORTCUT_CONFLICT: ErrorCode =
        ErrorCode::from_static("ui.plugin_shortcut_conflict");

    /// A clip drag cannot become an edit: it would leave the sequence, cross
    /// into a track of another kind, or touch a locked track. The `reason`
    /// detail carries the [`MoveRefusal`](crate::selection::MoveRefusal) id.
    pub const CLIP_MOVE_REFUSED: ErrorCode = ErrorCode::from_static("ui.clip_move_refused");

    /// A clip trim drag cannot become an edit: the clip is on a locked track
    /// or has left the sequence. Running out of source is not here — a trim is
    /// clamped to the source rather than refused. The `reason` detail carries
    /// the [`TrimRefusal`](crate::trim::TrimRefusal) id.
    pub const CLIP_TRIM_REFUSED: ErrorCode = ErrorCode::from_static("ui.clip_trim_refused");

    /// A cut cannot become an edit: the razor is on a locked track, the clip
    /// it named has gone, or the cut point is not inside that clip. The
    /// `reason` detail carries the [`SplitRefusal`](crate::split::SplitRefusal)
    /// id.
    pub const CLIP_SPLIT_REFUSED: ErrorCode = ErrorCode::from_static("ui.clip_split_refused");

    /// A fade handle drag cannot become an edit: the clip is on a locked
    /// track, it has left the sequence, or the fade is not representable. The
    /// `reason` detail carries the [`FadeRefusal`](crate::fade::FadeRefusal)
    /// id.
    pub const CLIP_FADE_REFUSED: ErrorCode = ErrorCode::from_static("ui.clip_fade_refused");
    /// An item from the bin cannot be edited onto a track: the track is
    /// locked or of another kind, or the item has not been probed. The
    /// `reason` detail carries the
    /// [`SourceRefusal`](crate::source_edit::SourceRefusal) id.
    pub const SOURCE_EDIT_REFUSED: ErrorCode = ErrorCode::from_static("ui.source_edit_refused");

    /// A crossfade drag cannot become an edit: the cut is on a locked track,
    /// the transition has left the sequence, or the drag would leave no blend
    /// at all. Running out of handle is not here — a crossfade is clamped to
    /// the handles rather than refused. The `reason` detail carries the
    /// [`TransitionRefusal`](crate::transition::TransitionRefusal) id.
    pub const TRANSITION_REFUSED: ErrorCode = ErrorCode::from_static("ui.transition_refused");

    /// A plugin's install directory could not be handed to the platform's
    /// file manager.
    pub const PLUGIN_FOLDER_UNOPENABLE: ErrorCode =
        ErrorCode::from_static("ui.plugin_folder_unopenable");

    /// The export panel cannot build a request yet: no preset, no sequence,
    /// no output file, or a range with no frames in it. The `field` detail
    /// names what is missing.
    pub const EXPORT_NOT_READY: ErrorCode = ErrorCode::from_static("ui.export_not_ready");

    /// A finished export's directory could not be handed to the platform's
    /// file manager.
    pub const EXPORT_FOLDER_UNOPENABLE: ErrorCode =
        ErrorCode::from_static("ui.export_folder_unopenable");

    /// A second export was asked for while one was already running. Only one
    /// runs at a time: they saturate the encoder, and a second one started by
    /// accident would fight the first for it.
    pub const EXPORT_BUSY: ErrorCode = ErrorCode::from_static("ui.export_busy");

    /// The chosen preset came from an exporter plugin, which the host cannot
    /// resolve to export settings yet. The `preset` and `plugin` details name
    /// it.
    pub const EXPORT_PRESET_UNSUPPORTED: ErrorCode =
        ErrorCode::from_static("ui.export_preset_unsupported");
}
