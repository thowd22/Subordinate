//! Rendering a sequence to a file through GStreamer encoders and muxers.
//!
//! Probes encoder availability (NVENC, VA-API, AMF, VideoToolbox, Media
//! Foundation, x264) and drives an appsrc-based pipeline with progress and
//! cancellation. See docs/PLAN.md §5.5.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_links() {}
}
