//! Media I/O built on GStreamer: probing, decoding, seeking, thumbnails and proxies.
//!
//! Prefers hardware decoders (nvdec, va, vtdec, d3d12) and yields NV12 frames
//! with rational PTS. Owns the frame cache and the PTS index used for
//! variable-frame-rate sources. See docs/PLAN.md §5.2.

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
}
