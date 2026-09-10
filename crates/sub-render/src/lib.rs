//! GPU compositor on wgpu: frame graph, NV12 conversion, transforms and shader effects.
//!
//! The same graph serves the preview (may drop frames) and export (never
//! drops). Plugin effects are WGSL shaders plus a parameter schema compiled
//! and cached here. See docs/PLAN.md §5.3.
//!
//! Everything hangs off a [`RenderContext`], the one wgpu device the egui UI
//! and the compositor share so preview textures need no copies (§3).

pub mod adapter;
pub mod context;
pub mod effect;
pub mod error;
pub mod graph;
pub mod nv12;
pub mod readback;

pub use adapter::{
    AdapterRank, adapter_rank, backend_label, best_adapter_index, describe_adapter,
    device_type_label, select_adapter,
};
pub use context::RenderContext;
pub use effect::{
    EffectCache, EffectDesc, EffectFailure, EffectInstance, EffectKey, EffectParam, EffectPipeline,
    PARAM_SLOT_BYTES, ParamKind, ParamValue, UniformField, UniformLayout,
};
pub use error::RenderError;
pub use graph::{
    Compositor, EffectSource, FrameSource, FrameSummary, LayerEffectFailure, LayerSummary,
    LetterboxFit, NoEffects, QuadTransform, ResolvedClip, SourceFrame, resolve_clip_at,
    resolve_layers_at,
};
pub use nv12::{Nv12Converter, Nv12Geometry, OUTPUT_FORMAT};
pub use readback::{
    BYTES_PER_PIXEL, DEFAULT_DEPTH, FrameBuffer, FrameReadback, StagingRing, padded_row_bytes,
};
