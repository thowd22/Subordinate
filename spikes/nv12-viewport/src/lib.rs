//! TASK-7 spike: H.264 -> GStreamer -> NV12 -> wgpu -> egui, with a pop-out
//! viewport on a second display.
//!
//! Throwaway code by design (the task is labelled `spike`): the deliverable is
//! the findings doc, not this crate. It is kept in the workspace only so the
//! numbers can be reproduced and so the pure parts — decoder preference, NV12
//! geometry, the newest-wins hand-off — stay covered by `cargo test` while the
//! real implementations (TASK-14, TASK-20, TASK-67) are written against them.
//!
//! ```text
//! spike-nv12-viewport bench [--width W --height H --frames N]
//! spike-nv12-viewport play <file> [--no-pop-out] [--frames N]
//! ```
//!
//! `bench` needs only a wgpu adapter (a software one will do) and measures the
//! upload and conversion path on synthesised 4K frames. `play` additionally
//! needs a display and a GStreamer installation with H.264 decoders.

pub mod app;
pub mod bench;
pub mod decode;
pub mod frames;
pub mod nv12;

/// Stable [`sub_core::ErrorCode`] constants this spike returns.
///
/// The `spike` domain says plainly that these are not part of the product's
/// error contract: the real codes are minted in `sub-media` and `sub-render`.
pub mod codes {
    use sub_core::ErrorCode;

    /// The picture size or a plane stride is not a valid NV12 layout.
    pub const BAD_GEOMETRY: ErrorCode = ErrorCode::from_static("spike.bad_geometry");
    /// A plane slice is too short for the geometry it claims.
    pub const SHORT_PLANE: ErrorCode = ErrorCode::from_static("spike.short_plane");
    /// GStreamer would not initialise.
    pub const INIT_FAILED: ErrorCode = ErrorCode::from_static("spike.init_failed");
    /// An element is missing, or the pipeline could not be wired.
    pub const PIPELINE_BUILD: ErrorCode = ErrorCode::from_static("spike.pipeline_build");
    /// The pipeline would not go to `Playing`.
    pub const PIPELINE_START: ErrorCode = ErrorCode::from_static("spike.pipeline_start");
    /// The pipeline posted an error on its bus while running.
    pub const PIPELINE_FAILED: ErrorCode = ErrorCode::from_static("spike.pipeline_failed");
    /// Negotiated caps the upload path cannot use.
    pub const BAD_CAPS: ErrorCode = ErrorCode::from_static("spike.bad_caps");
    /// A sample arrived with no usable buffer.
    pub const BAD_BUFFER: ErrorCode = ErrorCode::from_static("spike.bad_buffer");
    /// No wgpu adapter at all.
    pub const NO_DEVICE: ErrorCode = ErrorCode::from_static("spike.no_device");
    /// The GPU stopped responding mid-measurement.
    pub const GPU_LOST: ErrorCode = ErrorCode::from_static("spike.gpu_lost");
    /// The command line could not be understood.
    pub const BAD_ARGUMENTS: ErrorCode = ErrorCode::from_static("spike.bad_arguments");
    /// The window could not be opened.
    pub const WINDOW_FAILED: ErrorCode = ErrorCode::from_static("spike.window_failed");
}
