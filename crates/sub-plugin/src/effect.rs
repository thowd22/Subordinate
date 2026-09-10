//! Lifting an `effect` plugin's declaration into the compositor's own types.
//!
//! A WASM guest never touches the GPU (decision-6): an effect plugin exports
//! `describe`, which hands back a `effect-desc` — a parameter list, WGSL and
//! a fragment entry point — and the host compiles and runs it. This module is
//! the one place the WIT vocabulary meets `sub-render`'s, exactly as
//! [`crate::convert`] is for the model types.
//!
//! It lives behind the `render` feature so the plugin host stays free of wgpu
//! for the headless consumers — `subordinate-mcp` loads plugins and never
//! opens a device — while the editor, which has both, gets the conversion.
//!
//! A plugin is untrusted input, so every conversion is fallible: a parameter
//! id that is not an identifier, a range that does not hold its default, an
//! empty shader or an entry point that is not a name all come back as a
//! [`SubError`] carrying the render layer's own stable code.

use sub_core::{ErrorCode, SubError, SubResult};
use sub_render::{EffectDesc, EffectInstance, EffectParam, ParamKind, ParamValue, RenderError};

use crate::codes;
use crate::effect_types as wit;

/// Lift a plugin's `describe` result into a validated [`EffectDesc`].
///
/// # Errors
///
/// The render layer's own codes — `render.invalid_effect_param` and
/// `render.invalid_effect_shader` — when the declaration cannot be bound. The
/// WGSL itself is compiled later, by the compositor's effect cache.
pub fn effect_desc(desc: &wit::EffectDesc) -> SubResult<EffectDesc> {
    let params = desc.params.iter().map(param_desc).collect();
    EffectDesc::new(params, desc.shader.clone(), desc.entry.clone())
        .map_err(|error| lift_render_error(&error))
}

/// Lift one `param-desc` record.
pub fn param_desc(param: &wit::ParamDesc) -> EffectParam {
    EffectParam::new(
        param.id.clone(),
        param.label.clone(),
        param_kind(&param.kind),
    )
    .with_doc(param.doc.clone())
}

/// Lift one `param-kind` variant.
pub fn param_kind(kind: &wit::ParamKind) -> ParamKind {
    match kind {
        wit::ParamKind::Float(range) => ParamKind::Float {
            min: range.min,
            max: range.max,
            default: range.default,
            step: range.step,
        },
        wit::ParamKind::Int(range) => ParamKind::Int {
            min: range.min,
            max: range.max,
            default: range.default,
        },
        wit::ParamKind::Boolean(flag) => ParamKind::Bool {
            default: flag.default,
        },
        wit::ParamKind::Color(colour) => ParamKind::Color {
            default: channels(colour.default),
        },
        wit::ParamKind::Choice(choice) => ParamKind::Choice {
            variants: choice
                .variants
                .iter()
                .map(|variant| variant.id.clone())
                .collect(),
            default: choice.default,
        },
    }
}

/// Lift one `param-value` variant.
pub fn param_value(value: &wit::ParamValue) -> ParamValue {
    match *value {
        wit::ParamValue::Float(value) => ParamValue::Float(value),
        wit::ParamValue::Int(value) => ParamValue::Int(value),
        wit::ParamValue::Boolean(value) => ParamValue::Bool(value),
        wit::ParamValue::Color(colour) => ParamValue::Color(channels(colour)),
        wit::ParamValue::Choice(index) => ParamValue::Choice(index),
    }
}

/// Bind `desc` with the values in `bindings`, keyed by parameter id.
///
/// A binding the declaration does not name is kept but never reaches the
/// shader, and a parameter with no binding takes its declared default, so a
/// project that outlives an edit to a plugin's parameter list still renders.
pub fn effect_instance(
    desc: &std::sync::Arc<EffectDesc>,
    bindings: &[wit::ParamBinding],
) -> EffectInstance {
    let mut instance = EffectInstance::new(std::sync::Arc::clone(desc));
    for binding in bindings {
        instance.set(binding.id.clone(), param_value(&binding.value));
    }
    instance
}

/// An RGBA record as the four channels the shader reads.
fn channels(colour: wit::Color) -> [f32; 4] {
    [colour.r, colour.g, colour.b, colour.a]
}

/// Carry a [`RenderError`] across as a [`SubError`], keeping its stable code.
fn lift_render_error(error: &RenderError) -> SubError {
    let code = ErrorCode::parse(error.code()).unwrap_or(codes::INVALID_EFFECT_DECLARATION);
    SubError::new(code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{effect_desc, effect_instance, param_value};
    use crate::effect_types as wit;
    use std::sync::Arc;
    use sub_render::{ParamKind, ParamValue};

    const SHADER: &str = "@fragment
fn fs_tint(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(source, source_sampler, in.uv);
    return vec4<f32>(mix(texel.rgb, params.tint.rgb, params.amount), texel.a);
}
";

    fn declaration() -> wit::EffectDesc {
        wit::EffectDesc {
            params: vec![
                wit::ParamDesc {
                    id: "amount".to_owned(),
                    label: "Amount".to_owned(),
                    doc: "How far towards the tint".to_owned(),
                    kind: wit::ParamKind::Float(wit::FloatParam {
                        min: 0.0,
                        max: 1.0,
                        default: 0.25,
                        step: Some(0.05),
                    }),
                },
                wit::ParamDesc {
                    id: "tint".to_owned(),
                    label: "Tint".to_owned(),
                    doc: String::new(),
                    kind: wit::ParamKind::Color(wit::ColorParam {
                        default: wit::Color {
                            r: 1.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        },
                    }),
                },
            ],
            shader: SHADER.to_owned(),
            entry: "fs_tint".to_owned(),
        }
    }

    #[test]
    fn a_declaration_crosses_with_its_layout() {
        let desc = effect_desc(&declaration()).expect("the declaration is valid");
        assert_eq!(desc.entry(), "fs_tint");
        assert_eq!(desc.params().len(), 2);
        assert_eq!(desc.params()[0].doc, "How far towards the tint");
        assert_eq!(
            desc.params()[0].kind,
            ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.25,
                step: Some(0.05),
            }
        );
        assert_eq!(
            desc.params()[1].kind,
            ParamKind::Color {
                default: [1.0, 0.0, 0.0, 1.0],
            }
        );
        // Two parameters, one 16-byte slot each.
        assert_eq!(desc.layout().size(), 32);
        assert_eq!(desc.layout().fields()[1].offset, 16);
    }

    #[test]
    fn a_bad_declaration_carries_the_render_code() {
        let mut broken = declaration();
        broken.params[0].id = "Amount".to_owned();
        let error = effect_desc(&broken).expect_err("an upper-case id is refused");
        assert_eq!(error.code.as_str(), "render.invalid_effect_param");

        let mut empty = declaration();
        empty.shader = String::new();
        let error = effect_desc(&empty).expect_err("an empty shader is refused");
        assert_eq!(error.code.as_str(), "render.invalid_effect_shader");
    }

    #[test]
    fn bindings_land_on_the_declared_parameters() {
        let desc = Arc::new(effect_desc(&declaration()).expect("valid"));
        let instance = effect_instance(
            &desc,
            &[wit::ParamBinding {
                id: "amount".to_owned(),
                value: wit::ParamValue::Float(0.75),
            }],
        );
        assert_eq!(
            instance.values().get("amount"),
            Some(&ParamValue::Float(0.75))
        );
        // The unbound colour keeps the declared default.
        assert_eq!(
            instance.values().get("tint"),
            Some(&ParamValue::Color([1.0, 0.0, 0.0, 1.0]))
        );
        let bytes = instance.uniform_bytes();
        let bound = f32::from_le_bytes(bytes[0..4].try_into().expect("four bytes of a slot"));
        assert!(
            (bound - 0.75).abs() <= f32::EPSILON,
            "the bound amount reached the shader as {bound}"
        );
    }

    #[test]
    fn every_value_case_crosses() {
        assert_eq!(
            param_value(&wit::ParamValue::Boolean(true)),
            ParamValue::Bool(true)
        );
        assert_eq!(param_value(&wit::ParamValue::Int(-3)), ParamValue::Int(-3));
        assert_eq!(
            param_value(&wit::ParamValue::Choice(2)),
            ParamValue::Choice(2)
        );
    }
}
