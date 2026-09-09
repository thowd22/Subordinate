//! Rendering a sequence to a file through GStreamer encoders and muxers.
//!
//! Probes encoder availability (NVENC, VA-API, AMF, VideoToolbox, Media
//! Foundation, x264) and drives an appsrc-based pipeline with progress and
//! cancellation. See docs/PLAN.md §5.5.
//!
//! The capability probe lives in [`encoder`]: it instantiates every encoder
//! the plan names, keeps the ones that reach `READY`, and picks per codec in
//! the plan's order unless the user pinned one in settings.

pub mod encoder;

pub use encoder::{
    CODECS, ElementProbe, EncoderPreferences, EncoderProbe, EncoderStatus, EncoderVendor,
    VideoCodec, encoder_names,
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
