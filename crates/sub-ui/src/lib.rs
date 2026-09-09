//! The egui application: timeline, media bin, viewer, inspector and export panels.
//!
//! The UI thread never blocks on media. The viewer can pop out into its own
//! viewport for a second display. See docs/PLAN.md §5.7.
//!
//! eframe is configured for its wgpu backend, so the device egui paints with
//! is the same device `sub-render` composites with; see [`app`].

pub mod app;
pub mod diagnostics;
pub mod timeline;
pub mod viewer;

pub use app::{AppOptions, SubordinateApp, run};
pub use diagnostics::DiagnosticsPanel;
pub use timeline::{ClipPlacement, TimelineView, TrackLayout, ZoomLevel};
pub use viewer::{ViewerAction, ViewerFit, ViewerFrame, ViewerPanel, ViewerState};

/// The stable error codes this crate reports.
pub mod codes {
    use sub_core::ErrorCode;

    /// A zoom level falls outside the timeline's zoom ladder.
    pub const INVALID_ZOOM: ErrorCode = ErrorCode::from_static("ui.invalid_zoom");
}
