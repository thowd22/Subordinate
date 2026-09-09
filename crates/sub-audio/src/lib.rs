//! Audio decode, resampling, the lock-free mixer graph and cpal output.
//!
//! The real-time audio callback never locks or allocates. The audio clock is
//! the playback master; video follows it. See docs/PLAN.md §5.4.
//!
//! Audio-only files are decoded here with symphonia rather than through a
//! GStreamer pipeline (decision-4): see [`decode`].

pub mod decode;
pub mod mixer;
pub mod resample;

pub use decode::{AudioInfo, Block, FileDecoder, Pcm, decode_file, probe_audio};
pub use mixer::{
    ClipSpec, MixGraph, MixGraphBuilder, Mixer, MixerConfig, MixerControl, TrackSpec, mixer,
};
pub use resample::{PcmReader, PcmWriter, ResampleStage, Resampler, pcm_ring};

/// Stable [`sub_core::ErrorCode`] constants this crate returns.
///
/// Codes are part of the contract with agents and plugins: an existing one is
/// never renamed or given a new meaning.
pub mod codes {
    use sub_core::ErrorCode;

    /// The file is missing, unreadable, or not a file at all.
    pub const FILE_UNREADABLE: ErrorCode = ErrorCode::from_static("audio.file_unreadable");
    /// The container or codec is not one this build can decode.
    pub const UNSUPPORTED: ErrorCode = ErrorCode::from_static("audio.unsupported");
    /// The file was understood but its samples could not be decoded.
    pub const DECODE_FAILED: ErrorCode = ErrorCode::from_static("audio.decode_failed");
    /// A seek could not be satisfied by the container.
    pub const SEEK_FAILED: ErrorCode = ErrorCode::from_static("audio.seek_failed");
    /// The file carries no audio track with a known codec.
    pub const NO_AUDIO_TRACK: ErrorCode = ErrorCode::from_static("audio.no_audio_track");
    /// The track declares a shape this decoder cannot represent: no sample
    /// rate, no channels, or more channels than a mixer bus can hold.
    pub const UNSUPPORTED_LAYOUT: ErrorCode = ErrorCode::from_static("audio.unsupported_layout");
    /// A sample rate conversion could not be built or could not run.
    pub const RESAMPLE_FAILED: ErrorCode = ErrorCode::from_static("audio.resample_failed");
    /// A gain is not a finite level within the fader's range.
    pub const INVALID_GAIN: ErrorCode = ErrorCode::from_static("audio.invalid_gain");
    /// A mixer graph describes something the mixer cannot play: a negative or
    /// unrepresentable time, fades longer than their clip, or a shape the
    /// mixer it is published to does not have room for.
    pub const GRAPH_INVALID: ErrorCode = ErrorCode::from_static("audio.graph_invalid");
}

#[cfg(test)]
mod tests {
    #[test]
    fn error_codes_are_well_formed_and_distinct() {
        let codes = [
            super::codes::FILE_UNREADABLE,
            super::codes::UNSUPPORTED,
            super::codes::DECODE_FAILED,
            super::codes::SEEK_FAILED,
            super::codes::NO_AUDIO_TRACK,
            super::codes::UNSUPPORTED_LAYOUT,
            super::codes::RESAMPLE_FAILED,
            super::codes::INVALID_GAIN,
            super::codes::GRAPH_INVALID,
        ];
        for code in &codes {
            assert_eq!(code.domain(), "audio", "wrong domain for {code}");
        }
        let mut seen: Vec<&str> = codes.iter().map(sub_core::ErrorCode::as_str).collect();
        seen.sort_unstable();
        let total = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), total, "audio error codes must be unique");
    }
}
