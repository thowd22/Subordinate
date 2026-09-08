//! The egui application: timeline, media bin, viewer, inspector and export panels.
//!
//! The UI thread never blocks on media. The viewer can pop out into its own
//! viewport for a second display. See docs/PLAN.md §5.7.
//!
//! eframe is configured for its wgpu backend, so the device egui paints with
//! is the same device `sub-render` composites with; see [`app`].

pub mod app;

pub use app::{AppOptions, SubordinateApp, run};
