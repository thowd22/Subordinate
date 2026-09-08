//! Errors raised while setting up or driving the GPU compositor.
//!
//! Every variant carries a stable string code. When the shared `SubError`
//! type lands (TASK-11) these codes become its `code` field verbatim, so they
//! must not be renamed once released.

use core::fmt;

/// A failure in the render layer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RenderError {
    /// No wgpu adapter could be found for the requested backends.
    NoAdapter {
        /// Human-readable list of backends that were searched.
        backends: String,
    },
    /// An adapter was found but refused to hand out a device.
    DeviceRequestFailed {
        /// Adapter that refused, as `name (Backend)`.
        adapter: String,
        /// The underlying wgpu message.
        reason: String,
    },
    /// eframe was started without a wgpu render state.
    ///
    /// This means the `wgpu` feature of eframe was disabled or the glow
    /// backend was selected; the compositor cannot then share the UI device.
    MissingRenderState,
}

impl RenderError {
    /// The stable error code for this failure.
    ///
    /// These strings are part of the public API: they appear in Command API
    /// responses and in logs, and tools match on them.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoAdapter { .. } => "render.no_adapter",
            Self::DeviceRequestFailed { .. } => "render.device_request_failed",
            Self::MissingRenderState => "render.missing_render_state",
        }
    }
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter { backends } => {
                write!(f, "no GPU adapter available for backends [{backends}]")
            }
            Self::DeviceRequestFailed { adapter, reason } => {
                write!(f, "adapter {adapter} refused a device request: {reason}")
            }
            Self::MissingRenderState => {
                f.write_str("the UI was started without a wgpu render state")
            }
        }
    }
}

impl core::error::Error for RenderError {}

#[cfg(test)]
mod tests {
    use super::RenderError;

    #[test]
    fn codes_are_stable() {
        assert_eq!(
            RenderError::NoAdapter {
                backends: "Vulkan".to_owned()
            }
            .code(),
            "render.no_adapter"
        );
        assert_eq!(
            RenderError::DeviceRequestFailed {
                adapter: "llvmpipe (Vulkan)".to_owned(),
                reason: "out of memory".to_owned(),
            }
            .code(),
            "render.device_request_failed"
        );
        assert_eq!(
            RenderError::MissingRenderState.code(),
            "render.missing_render_state"
        );
    }

    #[test]
    fn display_mentions_the_cause() {
        let err = RenderError::NoAdapter {
            backends: "Vulkan | GL".to_owned(),
        };
        assert_eq!(
            err.to_string(),
            "no GPU adapter available for backends [Vulkan | GL]"
        );
    }
}
