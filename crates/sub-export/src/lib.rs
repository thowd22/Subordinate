//! Rendering a sequence to a file through GStreamer encoders and muxers.
//!
//! Probes encoder availability (NVENC, VA-API, AMF, VideoToolbox, Media
//! Foundation, x264) and drives an appsrc-based pipeline with progress and
//! cancellation. See docs/PLAN.md §5.5.
//!
//! The capability probe lives in [`encoder`]: it instantiates every encoder
//! the plan names, keeps the ones that reach `READY`, and picks per codec in
//! the plan's order unless the user pinned one in settings.

//! The pipeline itself lives in [`pipeline`]: two `appsrc` elements — the
//! composited frames and the offline audio mix — feeding the chosen encoders
//! and the container's muxer, with every timestamp computed exactly from the
//! sequence's rational frame rate.

//! The job layer lives in [`job`]: the loop around the pipeline that reports
//! frames done, an ETA and encoder statistics, stops on a cancel token, and
//! deletes the part-written file when an export does not reach its end.

pub mod encoder;
pub mod job;
pub mod pipeline;

pub use encoder::{
    CODECS, ElementProbe, EncoderPreferences, EncoderProbe, EncoderStatus, EncoderVendor,
    VideoCodec, element_is_usable, encoder_names,
};
pub use job::{
    DEFAULT_PROGRESS_INTERVAL, EXPORT_JOB_KIND, EncoderStats, ExportEvent, ExportJob,
    ExportJobHandle, ExportProgress, spawn_export_job,
};
pub use pipeline::{
    AUDIO_CODECS, AudioCodec, AudioFrameSource, BYTES_PER_PIXEL, CONTAINERS, Container,
    ExportElements, ExportPipeline, ExportReport, ExportSettings, PcmAudioSource, SolidFrames,
    VideoFrameSource, export, export_with,
};

/// Stable [`sub_core::ErrorCode`] constants this crate returns.
///
/// Codes are part of the contract with agents and plugins: an existing one is
/// never renamed or given a new meaning.
pub mod codes {
    use sub_core::ErrorCode;

    /// GStreamer itself could not be initialised.
    pub const INIT_FAILED: ErrorCode = ErrorCode::from_static("export.init_failed");
    /// The encoder probe could not be reported.
    pub const PROBE_FAILED: ErrorCode = ErrorCode::from_static("export.probe_failed");
    /// No encoder for the requested codec is usable on this machine.
    pub const NO_ENCODER: ErrorCode = ErrorCode::from_static("export.no_encoder");
    /// An encoder was named that the exporter does not know for that codec.
    pub const UNKNOWN_ENCODER: ErrorCode = ErrorCode::from_static("export.unknown_encoder");
    /// The user pinned an encoder this machine cannot use.
    pub const ENCODER_UNAVAILABLE: ErrorCode = ErrorCode::from_static("export.encoder_unavailable");
    /// The export settings do not describe a file that can be written.
    pub const INVALID_SETTINGS: ErrorCode = ErrorCode::from_static("export.invalid_settings");
    /// The container cannot carry one of the chosen codecs.
    pub const UNSUPPORTED_COMBINATION: ErrorCode =
        ErrorCode::from_static("export.unsupported_combination");
    /// The muxer the container needs is missing on this machine.
    pub const MUXER_UNAVAILABLE: ErrorCode = ErrorCode::from_static("export.muxer_unavailable");
    /// The export pipeline could not be built, linked, started or completed.
    pub const PIPELINE_FAILED: ErrorCode = ErrorCode::from_static("export.pipeline_failed");
    /// The pipeline refused a buffer, which is how a failing encoder surfaces
    /// in the middle of an export.
    pub const PUSH_FAILED: ErrorCode = ErrorCode::from_static("export.push_failed");
    /// The pipeline never reached end of stream inside its time budget.
    pub const EXPORT_TIMEOUT: ErrorCode = ErrorCode::from_static("export.timeout");
}

#[cfg(test)]
mod tests {
    #[test]
    fn error_codes_are_well_formed_and_distinct() {
        let codes = [
            super::codes::INIT_FAILED,
            super::codes::PROBE_FAILED,
            super::codes::NO_ENCODER,
            super::codes::UNKNOWN_ENCODER,
            super::codes::ENCODER_UNAVAILABLE,
            super::codes::INVALID_SETTINGS,
            super::codes::UNSUPPORTED_COMBINATION,
            super::codes::MUXER_UNAVAILABLE,
            super::codes::PIPELINE_FAILED,
            super::codes::PUSH_FAILED,
            super::codes::EXPORT_TIMEOUT,
        ];
        for code in &codes {
            assert_eq!(code.domain(), "export", "wrong domain for {code}");
        }
        let mut seen: Vec<&str> = codes.iter().map(sub_core::ErrorCode::as_str).collect();
        seen.sort_unstable();
        let total = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), total, "export error codes must be unique");
    }
}
