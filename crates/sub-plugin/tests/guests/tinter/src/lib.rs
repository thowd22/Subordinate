//! A `subordinate:plugin/effect` plugin: one parameter and a fragment entry.
//!
//! The declaration is the whole of an effect plugin (decision-6): the host
//! compiles the WGSL and runs it, so what this guest exists to prove is that
//! the test harness (TASK-91) can take a declaration out of a component,
//! compile it and render a frame with it. The shader scales the picture by
//! `params.amount`, whose default is well under one, so a frame that came back
//! unchanged means the effect never ran.

wit_bindgen::generate!({
    path: "../../../../../wit",
    world: "effect",
});

// `EffectDesc` is hoisted to the crate root by the world's own `use`; the
// three records only the declaration mentions stay in the interface's module.
use crate::subordinate::plugin::effect_types::{FloatParam, ParamDesc, ParamKind};

struct Plugin;

export!(Plugin);

/// The fragment stage. The host prepends its own prelude: the `EffectParams`
/// uniform built from the parameter list, `source` with `source_sampler`, and
/// the `VsOut` the full-screen vertex stage hands over.
const SHADER: &str = "
@fragment
fn fs_scale(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(source, source_sampler, in.uv);
    return vec4<f32>(texel.rgb * params.amount, texel.a);
}
";

impl Guest for Plugin {
    fn describe() -> EffectDesc {
        EffectDesc {
            params: vec![ParamDesc {
                id: "amount".to_owned(),
                label: "Amount".to_owned(),
                doc: "How much of the picture survives.".to_owned(),
                kind: ParamKind::Float(FloatParam {
                    min: 0.0,
                    max: 1.0,
                    default: 0.25,
                    step: None,
                }),
            }],
            shader: SHADER.to_owned(),
            entry: "fs_scale".to_owned(),
        }
    }
}
