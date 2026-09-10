//! Plugin shader effects: declaration, uniform layout, compilation and cache.
//!
//! A WASM plugin never touches the GPU (decision-6). An `effect` plugin
//! *declares* itself instead — a list of parameters plus one WGSL source with
//! a fragment entry point — and this module is the host half of that bargain:
//! it validates the declaration, derives a uniform layout from the parameters,
//! prepends the bindings and the vertex stage the plugin is forbidden to
//! declare itself, compiles the result and caches it by hash (docs/PLAN.md
//! §5.3).
//!
//! The declaration types mirror the `effect-types` records in
//! `wit/subordinate-plugin.wit` one for one — [`EffectParam`] is `param-desc`,
//! [`ParamKind`] is `param-kind`, [`ParamValue`] is `param-value` — but hold
//! no WIT and no wasmtime, so the compositor stays a GPU crate and the plugin
//! host stays a sandbox crate. `sub-plugin`'s `render` feature carries the
//! conversion between the two vocabularies.
//!
//! # The uniform layout
//!
//! Every declared parameter gets one 16-byte slot, in declaration order, with
//! `@align(16) @size(16)` on the member so WGSL's uniform address-space rules
//! are satisfied whatever the mix of scalars and colours. That costs a few
//! bytes over a tightly packed block and buys a layout that cannot be got
//! wrong: the offset of parameter *n* is `16 * n`, on every backend.
//!
//! ```
//! use sub_render::{EffectDesc, EffectParam, ParamKind};
//!
//! let params = vec![EffectParam::new(
//!     "amount",
//!     "Amount",
//!     ParamKind::Float { min: 0.0, max: 1.0, default: 0.5, step: None },
//! )];
//! let shader = "@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
//!     return textureSample(source, source_sampler, in.uv) * params.amount;
//! }";
//! let effect = EffectDesc::new(params, shader, "fs").expect("a valid declaration");
//! assert_eq!(effect.layout().size(), 16);
//! assert!(effect.module_source().contains("var<uniform> params"));
//! ```

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::context::RenderContext;
use crate::error::RenderError;
use crate::nv12::OUTPUT_FORMAT;

/// Bytes one parameter occupies in the uniform block.
///
/// A whole `vec4<f32>` slot per parameter, whatever its type: see the module
/// docs for why the layout trades a few bytes for one that is identical on
/// every backend.
pub const PARAM_SLOT_BYTES: u64 = 16;

/// [`PARAM_SLOT_BYTES`] as a byte count, for slicing the packed block.
const SLOT: usize = 16;

/// Name of the vertex entry point the host declares for every effect.
const VERTEX_ENTRY: &str = "vs_effect";

/// Which kind of value a parameter holds, with its range and default.
///
/// Mirrors the WIT `param-kind` variant. `Bool` is WIT's `boolean` case.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamKind {
    /// A continuous value, e.g. an exposure stop.
    Float {
        /// Smallest accepted value, inclusive.
        min: f32,
        /// Largest accepted value, inclusive; never below `min`.
        max: f32,
        /// The value used until the user changes it; within range.
        default: f32,
        /// Increment a stepped control snaps to; `None` means continuous.
        step: Option<f32>,
    },
    /// A whole number, e.g. a sample count.
    Int {
        /// Smallest accepted value, inclusive.
        min: i32,
        /// Largest accepted value, inclusive; never below `min`.
        max: i32,
        /// The value used until the user changes it; within range.
        default: i32,
    },
    /// A flag. It reaches the shader as `0u` or `1u`.
    Bool {
        /// The value used until the user changes it.
        default: bool,
    },
    /// A linear RGBA colour, each channel in `0.0..=1.0`.
    Color {
        /// The value used until the user changes it.
        default: [f32; 4],
    },
    /// A closed set of choices. It reaches the shader as the chosen index.
    Choice {
        /// The choices, in inspector order; never empty.
        variants: Vec<String>,
        /// Index into `variants` of the default choice; always in bounds.
        default: u32,
    },
}

impl ParamKind {
    /// The WGSL type of this parameter's uniform member.
    pub fn wgsl_type(&self) -> &'static str {
        match self {
            Self::Float { .. } => "f32",
            Self::Int { .. } => "i32",
            Self::Bool { .. } | Self::Choice { .. } => "u32",
            Self::Color { .. } => "vec4<f32>",
        }
    }

    /// The declared default, as the value the host binds until the project
    /// carries one of its own.
    pub fn default_value(&self) -> ParamValue {
        match *self {
            Self::Float { default, .. } => ParamValue::Float(default),
            Self::Int { default, .. } => ParamValue::Int(default),
            Self::Bool { default } => ParamValue::Bool(default),
            Self::Color { default } => ParamValue::Color(default),
            Self::Choice { default, .. } => ParamValue::Choice(default),
        }
    }

    /// Bring `value` inside the declared range, or fall back to the default
    /// when the case does not match the declaration at all.
    ///
    /// A plugin declares the range; the project stores whatever a command
    /// last wrote. Coercing here means a stale or out-of-range value renders
    /// a defined picture rather than an undefined one.
    pub fn coerce(&self, value: &ParamValue) -> ParamValue {
        match (self, value) {
            (Self::Float { min, max, .. }, ParamValue::Float(value)) => {
                ParamValue::Float(value.clamp(*min, *max))
            }
            (Self::Int { min, max, .. }, ParamValue::Int(value)) => {
                ParamValue::Int((*value).clamp(*min, *max))
            }
            (Self::Bool { .. }, ParamValue::Bool(value)) => ParamValue::Bool(*value),
            (Self::Color { .. }, ParamValue::Color(channels)) => {
                ParamValue::Color(channels.map(|channel| channel.clamp(0.0, 1.0)))
            }
            (Self::Choice { variants, .. }, ParamValue::Choice(index)) => {
                let last = u32::try_from(variants.len().saturating_sub(1)).unwrap_or(u32::MAX);
                ParamValue::Choice((*index).min(last))
            }
            _ => self.default_value(),
        }
    }

    /// Check the declaration's own invariants.
    fn validate(&self, id: &str) -> Result<(), RenderError> {
        let invalid = |reason: String| RenderError::InvalidEffectParam {
            param: id.to_owned(),
            reason,
        };
        match self {
            Self::Float {
                min,
                max,
                default,
                step,
            } => {
                if !min.is_finite() || !max.is_finite() || !default.is_finite() {
                    return Err(invalid("a float range must be finite".to_owned()));
                }
                if max < min {
                    return Err(invalid(format!("max {max} is below min {min}")));
                }
                if default < min || default > max {
                    return Err(invalid(format!(
                        "default {default} is outside {min}..={max}"
                    )));
                }
                if let Some(step) = step
                    && (!step.is_finite() || *step <= 0.0)
                {
                    return Err(invalid(format!("step {step} is not positive")));
                }
                Ok(())
            }
            Self::Int { min, max, default } => {
                if max < min {
                    return Err(invalid(format!("max {max} is below min {min}")));
                }
                if default < min || default > max {
                    return Err(invalid(format!(
                        "default {default} is outside {min}..={max}"
                    )));
                }
                Ok(())
            }
            Self::Bool { .. } => Ok(()),
            Self::Color { default } => {
                if default
                    .iter()
                    .any(|channel| !channel.is_finite() || *channel < 0.0 || *channel > 1.0)
                {
                    return Err(invalid("a colour channel must be in 0.0..=1.0".to_owned()));
                }
                Ok(())
            }
            Self::Choice { variants, default } => {
                if variants.is_empty() {
                    return Err(invalid("a choice needs at least one variant".to_owned()));
                }
                let count = u32::try_from(variants.len()).unwrap_or(u32::MAX);
                if *default >= count {
                    return Err(invalid(format!(
                        "default variant {default} is outside 0..{count}"
                    )));
                }
                Ok(())
            }
        }
    }
}

/// A parameter's current value. The case matches the [`ParamKind`] the
/// parameter was declared with; mirrors the WIT `param-value` variant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamValue {
    /// A continuous value.
    Float(f32),
    /// A whole number.
    Int(i32),
    /// A flag.
    Bool(bool),
    /// A linear RGBA colour.
    Color([f32; 4]),
    /// Index into the declared variants.
    Choice(u32),
}

impl ParamValue {
    /// The 16 bytes this value occupies in its uniform slot.
    ///
    /// Little-endian, which is what every backend wgpu targets reads, and
    /// zero-padded to the slot so no stale bytes reach the shader.
    fn slot_bytes(self) -> [u8; SLOT] {
        let mut bytes = [0u8; SLOT];
        match self {
            Self::Float(value) => bytes[..4].copy_from_slice(&value.to_le_bytes()),
            Self::Int(value) => bytes[..4].copy_from_slice(&value.to_le_bytes()),
            Self::Bool(value) => {
                bytes[..4].copy_from_slice(&u32::from(value).to_le_bytes());
            }
            Self::Choice(index) => bytes[..4].copy_from_slice(&index.to_le_bytes()),
            Self::Color(channels) => {
                for (slot, channel) in bytes.chunks_exact_mut(4).zip(channels) {
                    slot.copy_from_slice(&channel.to_le_bytes());
                }
            }
        }
        bytes
    }
}

/// One parameter of an effect: what the inspector shows and what the shader
/// reads. Mirrors the WIT `param-desc` record.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectParam {
    /// Stable machine-readable key, also the uniform member's name.
    /// Lowercase `[a-z0-9_]`, starting with a letter.
    pub id: String,
    /// Human-readable label for the inspector.
    pub label: String,
    /// One-line explanation for tooltips; empty when there is none.
    pub doc: String,
    /// The value's type, range and default.
    pub kind: ParamKind,
}

impl EffectParam {
    /// Declare a parameter with no tooltip.
    pub fn new(id: impl Into<String>, label: impl Into<String>, kind: ParamKind) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            doc: String::new(),
            kind,
        }
    }

    /// The same parameter with a tooltip.
    #[must_use]
    pub fn with_doc(mut self, doc: impl Into<String>) -> Self {
        self.doc = doc.into();
        self
    }

    /// Check the id spelling and the kind's own invariants.
    fn validate(&self) -> Result<(), RenderError> {
        if !is_wgsl_identifier(&self.id) {
            return Err(RenderError::InvalidEffectParam {
                param: self.id.clone(),
                reason: "an id must be lowercase [a-z][a-z0-9_]*".to_owned(),
            });
        }
        self.kind.validate(&self.id)
    }
}

/// Where one parameter sits in the uniform block.
#[derive(Debug, Clone, PartialEq)]
pub struct UniformField {
    /// The parameter's id, which is the WGSL member name.
    pub id: String,
    /// Byte offset of the member from the start of the block.
    pub offset: u64,
    /// The declared kind, which decides how the slot is filled.
    pub kind: ParamKind,
}

/// The uniform block the host builds from a plugin's parameter list.
///
/// Both halves of the contract live here: [`UniformLayout::wgsl`] is the
/// struct declaration the shader sees, and [`UniformLayout::pack`] fills the
/// bytes that back it, so the two can never disagree.
#[derive(Debug, Clone, PartialEq)]
pub struct UniformLayout {
    fields: Vec<UniformField>,
    size: u64,
    wgsl: String,
}

impl UniformLayout {
    /// Derive the layout of `params`, which have already been validated.
    fn build(params: &[EffectParam]) -> Self {
        let mut fields = Vec::with_capacity(params.len());
        let mut wgsl = String::from("struct EffectParams {\n");
        for (index, param) in params.iter().enumerate() {
            let offset = PARAM_SLOT_BYTES * index as u64;
            wgsl.push_str("    @align(16) @size(16) ");
            wgsl.push_str(&param.id);
            wgsl.push_str(": ");
            wgsl.push_str(param.kind.wgsl_type());
            wgsl.push_str(",\n");
            fields.push(UniformField {
                id: param.id.clone(),
                offset,
                kind: param.kind.clone(),
            });
        }
        // A uniform block may not be empty, and WGSL rounds a uniform struct
        // up to 16 bytes anyway: an effect with no parameters gets one unused
        // slot rather than a special case in every caller.
        if params.is_empty() {
            wgsl.push_str("    @align(16) @size(16) _unused: vec4<f32>,\n");
        }
        wgsl.push_str("};\n");
        let size = PARAM_SLOT_BYTES * params.len().max(1) as u64;
        Self { fields, size, wgsl }
    }

    /// The parameters, in declaration order, with their offsets.
    pub fn fields(&self) -> &[UniformField] {
        &self.fields
    }

    /// Size of the whole block in bytes; never zero.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The `struct EffectParams { .. }` declaration the shader is compiled
    /// with.
    pub fn wgsl(&self) -> &str {
        &self.wgsl
    }

    /// Fill the block from `values`, keyed by parameter id.
    ///
    /// A parameter with no value, or one whose case does not match its
    /// declaration, takes the declared default; a value outside the declared
    /// range is clamped into it. The result is always [`UniformLayout::size`]
    /// bytes long.
    pub fn pack(&self, values: &BTreeMap<String, ParamValue>) -> Vec<u8> {
        let mut bytes = vec![0u8; usize::try_from(self.size).unwrap_or(usize::MAX)];
        for field in &self.fields {
            let value = values.get(&field.id).map_or_else(
                || field.kind.default_value(),
                |value| field.kind.coerce(value),
            );
            let start = usize::try_from(field.offset).unwrap_or(usize::MAX);
            if let Some(slot) = bytes.get_mut(start..start + SLOT) {
                slot.copy_from_slice(&value.slot_bytes());
            }
        }
        bytes
    }
}

/// The identity of a compiled effect: a hash of everything the pipeline is
/// built from.
///
/// Two plugins that ship the same shader, entry point and parameter layout
/// share one pipeline, and an edited shader is a different key rather than a
/// stale hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EffectKey([u8; 32]);

impl EffectKey {
    /// The raw digest.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for EffectKey {
    /// The first eight bytes as hex, which is what logs carry.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..8] {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// A validated effect declaration: parameters, WGSL and entry point.
///
/// Mirrors the WIT `effect-desc` record. Construction validates: an
/// [`EffectDesc`] that exists has unique, well-spelled parameter ids, ranges
/// that hold their defaults, a non-empty shader and an entry point that is a
/// WGSL identifier. Whether the WGSL itself *compiles* is a GPU question and
/// is answered by [`EffectCache::compile`].
#[derive(Debug, Clone, PartialEq)]
pub struct EffectDesc {
    params: Vec<EffectParam>,
    shader: String,
    entry: String,
    layout: UniformLayout,
    module: String,
    key: EffectKey,
}

impl EffectDesc {
    /// Validate a declaration and derive its uniform layout and cache key.
    ///
    /// # Errors
    ///
    /// [`RenderError::InvalidEffectParam`] when a parameter id is misspelled
    /// or repeated or its range does not hold its default, and
    /// [`RenderError::InvalidEffectShader`] when the source is empty or the
    /// entry point is not a WGSL identifier.
    pub fn new(
        params: Vec<EffectParam>,
        shader: impl Into<String>,
        entry: impl Into<String>,
    ) -> Result<Self, RenderError> {
        let shader = shader.into();
        let entry = entry.into();
        let mut seen = Vec::with_capacity(params.len());
        for param in &params {
            param.validate()?;
            if seen.contains(&param.id.as_str()) {
                return Err(RenderError::InvalidEffectParam {
                    param: param.id.clone(),
                    reason: "a parameter id is declared twice".to_owned(),
                });
            }
            seen.push(param.id.as_str());
        }
        if shader.trim().is_empty() {
            return Err(RenderError::InvalidEffectShader {
                reason: "the shader source is empty".to_owned(),
            });
        }
        if !is_wgsl_identifier(&entry) {
            return Err(RenderError::InvalidEffectShader {
                reason: format!("entry point {entry:?} is not a WGSL identifier"),
            });
        }
        if entry == VERTEX_ENTRY {
            return Err(RenderError::InvalidEffectShader {
                reason: format!("{VERTEX_ENTRY} is the host's own vertex entry point"),
            });
        }
        let layout = UniformLayout::build(&params);
        let module = format!("{}{}\n{shader}\n", layout.wgsl(), PRELUDE_WGSL);
        let mut hasher = blake3::Hasher::new();
        hasher.update(module.as_bytes());
        hasher.update(&[0]);
        hasher.update(entry.as_bytes());
        let key = EffectKey(*hasher.finalize().as_bytes());
        Ok(Self {
            params,
            shader,
            entry,
            layout,
            module,
            key,
        })
    }

    /// The declared parameters, in inspector order.
    pub fn params(&self) -> &[EffectParam] {
        &self.params
    }

    /// The plugin's own WGSL, as declared.
    pub fn shader(&self) -> &str {
        &self.shader
    }

    /// Name of the plugin's fragment entry point.
    pub fn entry(&self) -> &str {
        &self.entry
    }

    /// The uniform block derived from the parameters.
    pub fn layout(&self) -> &UniformLayout {
        &self.layout
    }

    /// The whole module as compiled: the host's uniform struct, bindings and
    /// vertex stage, then the plugin's source.
    pub fn module_source(&self) -> &str {
        &self.module
    }

    /// The cache key: a hash of the compiled module and the entry point.
    pub fn key(&self) -> EffectKey {
        self.key
    }

    /// The declared defaults, ready to be edited into an [`EffectInstance`].
    pub fn defaults(&self) -> BTreeMap<String, ParamValue> {
        self.params
            .iter()
            .map(|param| (param.id.clone(), param.kind.default_value()))
            .collect()
    }
}

/// One effect on one clip: a declaration plus the values this clip binds.
///
/// Cheap to clone — the declaration is shared — because the compositor
/// collects the instances of every layer before it touches the GPU.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectInstance {
    desc: Arc<EffectDesc>,
    values: BTreeMap<String, ParamValue>,
}

impl EffectInstance {
    /// Bind `desc` with every parameter at its declared default.
    pub fn new(desc: Arc<EffectDesc>) -> Self {
        let values = desc.defaults();
        Self { desc, values }
    }

    /// Set one parameter by id. An id the effect does not declare is kept but
    /// never reaches the shader, so a project that outlives a plugin's
    /// parameter list loses nothing.
    #[must_use]
    pub fn with_value(mut self, id: impl Into<String>, value: ParamValue) -> Self {
        self.values.insert(id.into(), value);
        self
    }

    /// Set one parameter by id, in place.
    pub fn set(&mut self, id: impl Into<String>, value: ParamValue) {
        self.values.insert(id.into(), value);
    }

    /// The declaration this instance binds.
    pub fn desc(&self) -> &EffectDesc {
        &self.desc
    }

    /// The bound values, keyed by parameter id.
    pub fn values(&self) -> &BTreeMap<String, ParamValue> {
        &self.values
    }

    /// The uniform block for this instance.
    pub fn uniform_bytes(&self) -> Vec<u8> {
        self.desc.layout().pack(&self.values)
    }
}

/// One effect that could not run, and why.
///
/// The compositor draws the layer *without* the effect and reports this, so a
/// plugin whose WGSL does not compile costs the user one visible error rather
/// than a black frame or a crashed preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectFailure {
    /// Position of the effect in the clip's chain.
    pub index: usize,
    /// The declaration's cache key, for logs.
    pub effect: EffectKey,
    /// The stable error code, e.g. `render.effect_compile_failed`.
    pub code: &'static str,
    /// The compiler's own message, which is what the inspector shows.
    pub message: String,
}

/// The host half of every effect module: the bindings a plugin may use and
/// the vertex stage it may not declare.
///
/// A plugin's fragment entry takes a `VsOut` and returns a `vec4<f32>`; it
/// reads `params`, `source` and `source_sampler` and binds nothing itself.
const PRELUDE_WGSL: &str = r"
@group(0) @binding(0) var<uniform> params: EffectParams;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_effect(@builtin(vertex_index) index: u32) -> VsOut {
    // 0,1,2,3 as a triangle strip: the full-screen quad's TL, TR, BL, BR.
    let corner = vec2<f32>(
        f32(index & 1u),
        f32(index >> 1u),
    );
    var out: VsOut;
    out.position = vec4<f32>(
        2.0 * corner.x - 1.0,
        1.0 - 2.0 * corner.y,
        0.0,
        1.0,
    );
    out.uv = corner;
    return out;
}
";

/// A compiled effect: one pipeline, ready to bind.
#[derive(Debug)]
pub struct EffectPipeline {
    pipeline: wgpu::RenderPipeline,
    uniform_size: u64,
}

impl EffectPipeline {
    /// The render pipeline, whose fragment stage is the plugin's entry point.
    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    /// Bytes the pipeline's uniform block expects.
    pub fn uniform_size(&self) -> u64 {
        self.uniform_size
    }
}

/// Compiled effect pipelines, keyed by [`EffectKey`].
///
/// A failure is cached too: a plugin whose shader does not compile is asked
/// once, not once per frame, and the message it failed with is handed back
/// every time so the inspector can keep showing it.
#[derive(Debug)]
pub struct EffectCache {
    context: RenderContext,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    entries: HashMap<EffectKey, Result<Arc<EffectPipeline>, RenderError>>,
}

impl EffectCache {
    /// Build an empty cache on the shared device.
    pub fn new(context: RenderContext) -> Self {
        let device = context.device();
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("effect params"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("effect source"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        Self {
            context,
            bind_group_layout,
            sampler,
            entries: HashMap::new(),
        }
    }

    /// The bind group layout every effect pipeline is built against.
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }

    /// The sampler effects read their input through.
    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    /// How many declarations have been compiled, successfully or not.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been compiled yet.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether `key` has already been decided, hit or failure.
    pub fn contains(&self, key: EffectKey) -> bool {
        self.entries.contains_key(&key)
    }

    /// Compile `desc`, or hand back the pipeline — or the failure — already
    /// cached under its key.
    ///
    /// # Errors
    ///
    /// [`RenderError::EffectCompileFailed`] carrying naga's own message when
    /// the WGSL is rejected. The failure is cached, so the message is stable
    /// across frames and the driver is asked only once.
    pub fn compile(&mut self, desc: &EffectDesc) -> Result<Arc<EffectPipeline>, RenderError> {
        if let Some(entry) = self.entries.get(&desc.key()) {
            return entry.clone();
        }
        let compiled = self.build(desc);
        self.entries.insert(desc.key(), compiled.clone());
        compiled
    }

    /// Build the pipeline for `desc`, capturing validation errors rather than
    /// letting wgpu's uncaptured-error handler abort the process.
    fn build(&self, desc: &EffectDesc) -> Result<Arc<EffectPipeline>, RenderError> {
        let device = self.context.device();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("effect"),
            source: wgpu::ShaderSource::Wgsl(desc.module_source().into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("effect"),
            bind_group_layouts: &[Some(&self.bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("effect"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some(VERTEX_ENTRY),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(desc.entry()),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    // The pass replaces its target: an effect owns every
                    // pixel of the picture it is handed, alpha included.
                    format: OUTPUT_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        match pollster::block_on(scope.pop()) {
            Some(error) => Err(RenderError::EffectCompileFailed {
                entry: desc.entry().to_owned(),
                message: error.to_string(),
            }),
            None => Ok(Arc::new(EffectPipeline {
                pipeline,
                uniform_size: desc.layout().size(),
            })),
        }
    }
}

/// Whether `text` is a WGSL identifier of the shape plugin ids are held to:
/// lowercase, starting with a letter, `[a-z][a-z0-9_]*`.
fn is_wgsl_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

#[cfg(test)]
mod tests {
    use super::{EffectDesc, EffectInstance, EffectParam, ParamKind, ParamValue, UniformLayout};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    const TINT: &str = "@fragment
fn fs_tint(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(source, source_sampler, in.uv);
    return vec4<f32>(mix(texel.rgb, params.tint.rgb, params.amount), texel.a);
}
";

    /// The `f32` at `offset` of a packed block.
    fn slot_f32(bytes: &[u8], offset: usize) -> f32 {
        f32::from_le_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("four bytes of a slot"),
        )
    }

    /// Assert a packed `f32` is `expected`; the packer copies bytes, so the
    /// only slack needed is the one ulp a comparison of equals may not use.
    fn assert_slot(bytes: &[u8], offset: usize, expected: f32) {
        let actual = slot_f32(bytes, offset);
        assert!(
            (actual - expected).abs() <= f32::EPSILON,
            "slot at {offset} was {actual}, expected {expected}"
        );
    }

    fn tint_params() -> Vec<EffectParam> {
        vec![
            EffectParam::new(
                "amount",
                "Amount",
                ParamKind::Float {
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    step: None,
                },
            ),
            EffectParam::new(
                "tint",
                "Tint",
                ParamKind::Color {
                    default: [1.0, 0.0, 0.0, 1.0],
                },
            ),
        ]
    }

    fn tint() -> EffectDesc {
        EffectDesc::new(tint_params(), TINT, "fs_tint").expect("the tint declaration is valid")
    }

    #[test]
    fn a_layout_gives_every_parameter_its_own_slot() {
        let desc = tint();
        let layout = desc.layout();
        assert_eq!(layout.size(), 32);
        assert_eq!(layout.fields()[0].offset, 0);
        assert_eq!(layout.fields()[1].offset, 16);
        assert!(layout.wgsl().contains("amount: f32"));
        assert!(layout.wgsl().contains("tint: vec4<f32>"));
    }

    #[test]
    fn an_effect_with_no_parameters_still_has_a_block() {
        let layout = UniformLayout::build(&[]);
        assert_eq!(layout.size(), 16);
        assert!(layout.wgsl().contains("_unused"));
    }

    #[test]
    fn packing_writes_defaults_clamps_values_and_zero_pads() {
        let desc = tint();
        let bytes = desc.layout().pack(&BTreeMap::new());
        assert_eq!(bytes.len(), 32);
        assert_slot(&bytes, 0, 0.5);
        // The bytes after a scalar are padding, not stale bytes.
        assert_eq!(&bytes[4..16], &[0u8; 12]);
        assert_slot(&bytes, 16, 1.0);

        let instance = EffectInstance::new(Arc::new(desc)).with_value(
            "amount",
            // Out of range: it must reach the shader clamped, not verbatim.
            ParamValue::Float(4.0),
        );
        let bytes = instance.uniform_bytes();
        assert_slot(&bytes, 0, 1.0);
    }

    #[test]
    fn a_mismatched_value_falls_back_to_the_default() {
        let desc = Arc::new(tint());
        let instance = EffectInstance::new(Arc::clone(&desc))
            .with_value("amount", ParamValue::Bool(true))
            .with_value("nonesuch", ParamValue::Int(3));
        let bytes = instance.uniform_bytes();
        assert_eq!(bytes.len(), 32);
        assert_slot(&bytes, 0, 0.5);
    }

    #[test]
    fn bool_and_choice_reach_the_shader_as_u32() {
        let params = vec![
            EffectParam::new("flag", "Flag", ParamKind::Bool { default: false }),
            EffectParam::new(
                "mode",
                "Mode",
                ParamKind::Choice {
                    variants: vec!["luma".to_owned(), "chroma".to_owned()],
                    default: 0,
                },
            ),
        ];
        let desc = EffectDesc::new(params, TINT, "fs_tint").expect("valid");
        let instance = EffectInstance::new(Arc::new(desc))
            .with_value("flag", ParamValue::Bool(true))
            // Out of bounds: clamped to the last variant.
            .with_value("mode", ParamValue::Choice(9));
        let bytes = instance.uniform_bytes();
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 1);
    }

    #[test]
    fn the_key_follows_the_shader_the_entry_and_the_layout() {
        let same = tint();
        assert_eq!(tint().key(), same.key());

        let edited =
            EffectDesc::new(tint_params(), format!("{TINT}\n// edited"), "fs_tint").expect("valid");
        assert_ne!(tint().key(), edited.key());

        let renamed = EffectDesc::new(
            tint_params(),
            TINT.replace("fs_tint", "fs_other"),
            "fs_other",
        )
        .expect("valid");
        assert_ne!(tint().key(), renamed.key());

        let mut fewer = tint_params();
        fewer.pop();
        let trimmed = EffectDesc::new(fewer, TINT, "fs_tint").expect("valid");
        assert_ne!(tint().key(), trimmed.key());
    }

    #[test]
    fn a_bad_declaration_is_refused_with_a_stable_code() {
        let repeated = EffectDesc::new(
            vec![
                EffectParam::new("amount", "A", ParamKind::Bool { default: false }),
                EffectParam::new("amount", "B", ParamKind::Bool { default: true }),
            ],
            TINT,
            "fs_tint",
        )
        .expect_err("a repeated id is refused");
        assert_eq!(repeated.code(), "render.invalid_effect_param");

        let shouted = EffectDesc::new(
            vec![EffectParam::new(
                "Amount",
                "A",
                ParamKind::Bool { default: false },
            )],
            TINT,
            "fs_tint",
        )
        .expect_err("an upper-case id is refused");
        assert_eq!(shouted.code(), "render.invalid_effect_param");

        let out_of_range = EffectDesc::new(
            vec![EffectParam::new(
                "amount",
                "A",
                ParamKind::Float {
                    min: 0.0,
                    max: 1.0,
                    default: 2.0,
                    step: None,
                },
            )],
            TINT,
            "fs_tint",
        )
        .expect_err("a default outside the range is refused");
        assert_eq!(out_of_range.code(), "render.invalid_effect_param");

        let empty =
            EffectDesc::new(Vec::new(), "   ", "fs_tint").expect_err("an empty shader is refused");
        assert_eq!(empty.code(), "render.invalid_effect_shader");

        let bad_entry =
            EffectDesc::new(Vec::new(), TINT, "1fs").expect_err("a bad entry point is refused");
        assert_eq!(bad_entry.code(), "render.invalid_effect_shader");

        let stolen = EffectDesc::new(Vec::new(), TINT, "vs_effect")
            .expect_err("the host's vertex entry is refused");
        assert_eq!(stolen.code(), "render.invalid_effect_shader");

        let no_variants = EffectDesc::new(
            vec![EffectParam::new(
                "mode",
                "Mode",
                ParamKind::Choice {
                    variants: Vec::new(),
                    default: 0,
                },
            )],
            TINT,
            "fs_tint",
        )
        .expect_err("an empty choice is refused");
        assert_eq!(no_variants.code(), "render.invalid_effect_param");
    }

    #[test]
    fn the_module_carries_the_bindings_the_plugin_may_not_declare() {
        let desc = tint();
        let module = desc.module_source();
        assert!(module.starts_with("struct EffectParams"));
        assert!(module.contains("@group(0) @binding(0) var<uniform> params: EffectParams;"));
        assert!(module.contains("fn vs_effect"));
        assert!(module.ends_with("}\n\n"));
    }
}
