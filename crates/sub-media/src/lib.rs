//! Media I/O built on GStreamer: probing, decoding, seeking, thumbnails and proxies.
//!
//! Prefers hardware decoders (nvdec, va, vtdec, d3d12) and yields NV12 frames
//! with rational PTS. Owns the frame cache and the PTS index used for
//! variable-frame-rate sources. See docs/PLAN.md §5.2.
//!
//! A video file is demuxed once, so the audio inside one is decoded here too,
//! off the same pipeline, as interleaved `f32` PCM (decision-4): see
//! [`audio`]. Audio-only files are read by `sub-audio` instead.

pub mod audio;
pub mod decode;
pub mod diagnostics;
pub mod index;
pub mod probe;

pub use audio::{
    AudioBlock, AudioChannels, AudioFormat, CENTER_DOWNMIX_GAIN, ChannelLayout, ChannelRole,
    LFE_DOWNMIX_GAIN, MAX_CHANNELS, SURROUND_DOWNMIX_GAIN,
};
pub use decode::{
    Decoder, DecoderOptions, FrameFormat, HardwarePreference, StreamSelection, VideoFrame,
};
pub use diagnostics::{
    ElementKind, ElementStatus, HardwareDiagnostics, VENDORS, Vendor, VendorReport,
};
pub use index::{
    CancelToken, INDEX_CACHE_VERSION, IndexEntry, IndexJob, IndexedDecoder, LazyPtsIndex, PtsIndex,
};
pub use probe::{
    FrameTiming, MediaInfo, ProbeOptions, Rotation, VideoStreamInfo, probe, probe_with,
};

/// Stable [`sub_core::ErrorCode`] constants this crate returns.
///
/// Codes are part of the contract with agents and plugins: an existing one is
/// never renamed or given a new meaning.
pub mod codes {
    use sub_core::ErrorCode;

    /// GStreamer itself could not be initialised.
    pub const INIT_FAILED: ErrorCode = ErrorCode::from_static("media.init_failed");
    /// The file is missing, unreadable, or not a file at all.
    pub const FILE_UNREADABLE: ErrorCode = ErrorCode::from_static("media.file_unreadable");
    /// The file was reached but could not be understood: a corrupt container,
    /// a truncated stream, a demuxer failure.
    pub const PROBE_FAILED: ErrorCode = ErrorCode::from_static("media.probe_failed");
    /// The file is understood but this installation cannot handle it: no
    /// demuxer, parser or decoder for the format.
    pub const UNSUPPORTED: ErrorCode = ErrorCode::from_static("media.unsupported");
    /// The probe ran out of its time budget.
    pub const PROBE_TIMEOUT: ErrorCode = ErrorCode::from_static("media.probe_timeout");
    /// A decode pipeline could not be built, started, or ran into an error
    /// while frames were being pulled from it.
    pub const DECODE_FAILED: ErrorCode = ErrorCode::from_static("media.decode_failed");
    /// A decode pipeline stalled: no frame arrived within the frame budget.
    pub const DECODE_TIMEOUT: ErrorCode = ErrorCode::from_static("media.decode_timeout");
    /// The file was opened for decoding but carries no video stream.
    pub const NO_VIDEO_STREAM: ErrorCode = ErrorCode::from_static("media.no_video_stream");
    /// A seek could not be performed: the pipeline refused it, or never
    /// reached a state in which it could be seeked.
    pub const SEEK_FAILED: ErrorCode = ErrorCode::from_static("media.seek_failed");
    /// The file was opened for audio decoding but carries no audio stream.
    pub const NO_AUDIO_STREAM: ErrorCode = ErrorCode::from_static("media.no_audio_stream");
    /// A PTS index could not be built, cached or read back: the parse pipeline
    /// failed, the file exposes no timed pictures, or the sidecar cache could
    /// not be written or understood.
    pub const INDEX_FAILED: ErrorCode = ErrorCode::from_static("media.index_failed");
}

/// Initialises GStreamer once and returns its runtime version.
///
/// Safe to call repeatedly; later calls are no-ops.
///
/// # Errors
///
/// Returns the GStreamer error if the library cannot be initialised.
pub fn init() -> Result<(u32, u32, u32, u32), gstreamer::glib::Error> {
    gstreamer::init()?;
    Ok(gstreamer::version())
}

#[cfg(test)]
mod tests {
    #[test]
    fn gstreamer_initialises() {
        let (major, minor, _, _) = super::init().expect("GStreamer must initialise");
        assert_eq!(major, 1);
        assert!(
            minor >= 24,
            "GStreamer 1.24 or newer required, got 1.{minor}"
        );
    }

    #[test]
    fn error_codes_are_well_formed_and_distinct() {
        let codes = [
            super::codes::INIT_FAILED,
            super::codes::FILE_UNREADABLE,
            super::codes::PROBE_FAILED,
            super::codes::UNSUPPORTED,
            super::codes::PROBE_TIMEOUT,
            super::codes::DECODE_FAILED,
            super::codes::DECODE_TIMEOUT,
            super::codes::NO_VIDEO_STREAM,
            super::codes::SEEK_FAILED,
            super::codes::NO_AUDIO_STREAM,
            super::codes::INDEX_FAILED,
        ];
        for code in &codes {
            assert_eq!(code.domain(), "media", "wrong domain for {code}");
        }
        let mut seen: Vec<&str> = codes.iter().map(sub_core::ErrorCode::as_str).collect();
        seen.sort_unstable();
        let total = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), total, "media error codes must be unique");
    }
}
