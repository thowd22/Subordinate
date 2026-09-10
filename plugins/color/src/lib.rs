//! The reference `effect` plugin: a colour grade with tint, exposure and
//! saturation.
//!
//! This is the worked example for `subordinate:plugin@0.1.0`'s `effect` world
//! (docs/PLAN.md §6.2) — the template a scaffolded effect grows into. A WASM
//! guest never touches the GPU (decision-6), so an effect plugin *declares*
//! itself and the host does the rest:
//!
//! - `describe()` returns one `effect-desc`: the parameter list the inspector
//!   shows, the WGSL source, and the name of the fragment entry point in it.
//!   It is called once when the host loads the plugin and again after a hot
//!   reload — never per frame — because an effect declaration is static.
//! - The host builds the uniform block from the parameters (one member per
//!   `id`, in declaration order), prepends its own bindings and vertex stage,
//!   compiles the module and caches it by hash, then runs it on each clip that
//!   carries the effect.
//! - The plugin has no say in a clip's parameter *values*: those live in the
//!   project model and are edited by ordinary undoable commands.
//!
//! So the plugin is two files and almost no code: [`grade`] is the parameter
//! table plus a CPU reference for what the shader does, and `src/effect.wgsl`
//! is the shader itself. `describe` below is only the lift from one to the
//! other.
//!
//! Build it with `cargo build --release --target wasm32-wasip2`; the Rust
//! toolchain emits a component directly, so there is no `cargo component` or
//! `wasm-tools` step.

use subordinate_sdk::bindings::EffectDesc;
use subordinate_sdk::bindings::subordinate::plugin::effect_types::{
    Color, ColorParam, FloatParam, ParamDesc, ParamKind,
};
use subordinate_sdk::{Guest, export};

pub mod grade;

/// Name of the fragment entry point in `src/effect.wgsl`.
pub const ENTRY: &str = "fs_main";

/// The plugin's WGSL, compiled by the host and never by the plugin.
pub const SHADER: &str = include_str!("effect.wgsl");

/// The effect's declaration: the parameter table of [`grade`] lifted into the
/// `effect-types` records, plus the shader that reads it.
///
/// The lift is the only place the two vocabularies meet, so a parameter added
/// to `PARAMS` reaches the inspector, the uniform block and the shader without
/// anything else being edited.
pub fn description() -> EffectDesc {
    EffectDesc {
        params: grade::PARAMS.iter().map(param_desc).collect(),
        shader: SHADER.to_owned(),
        entry: ENTRY.to_owned(),
    }
}

/// One row of the table as the WIT record the host reads.
fn param_desc(spec: &grade::ParamSpec) -> ParamDesc {
    ParamDesc {
        id: spec.id.to_owned(),
        label: spec.label.to_owned(),
        doc: spec.doc.to_owned(),
        kind: match spec.kind {
            grade::SpecKind::Float { min, max, default } => ParamKind::Float(FloatParam {
                min,
                max,
                default,
                // Continuous: an exposure or a saturation has no natural
                // increment, and a stepped control would only get in the way
                // of a fine adjustment.
                step: None,
            }),
            grade::SpecKind::Color { default } => ParamKind::Color(ColorParam {
                default: Color {
                    r: default[0],
                    g: default[1],
                    b: default[2],
                    a: default[3],
                },
            }),
        },
    }
}

/// The component's exports.
pub struct ColorGrade;

impl Guest for ColorGrade {
    fn describe() -> EffectDesc {
        description()
    }
}

export!(ColorGrade);

#[cfg(test)]
mod tests {
    use super::{ENTRY, SHADER, description, grade};
    use subordinate_sdk::bindings::subordinate::plugin::effect_types::ParamKind;

    #[test]
    fn the_declaration_carries_every_parameter_in_table_order() {
        let description = description();
        let declared: Vec<&str> = description
            .params
            .iter()
            .map(|param| param.id.as_str())
            .collect();
        let expected: Vec<&str> = grade::PARAMS.iter().map(|spec| spec.id).collect();
        assert_eq!(declared, expected);
        for param in &description.params {
            assert!(!param.label.is_empty());
            assert!(!param.doc.is_empty());
            // The id is also the uniform member's name, so it has to be a WGSL
            // identifier: lowercase, letter first, `[a-z0-9_]` after.
            let mut characters = param.id.chars();
            assert!(
                characters
                    .next()
                    .is_some_and(|first| first.is_ascii_lowercase())
            );
            assert!(
                characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{}",
                param.id
            );
        }
    }

    #[test]
    fn every_range_holds_its_default() {
        for param in description().params {
            match param.kind {
                ParamKind::Float(range) => {
                    assert!(range.min <= range.default, "{}", param.id);
                    assert!(range.default <= range.max, "{}", param.id);
                    assert!(range.step.is_none(), "{}", param.id);
                }
                ParamKind::Color(colour) => {
                    let channels = [
                        colour.default.r,
                        colour.default.g,
                        colour.default.b,
                        colour.default.a,
                    ];
                    assert!(channels.iter().all(|channel| (0.0..=1.0).contains(channel)));
                }
                other => panic!("{} declares an unexpected kind {other:?}", param.id),
            }
        }
    }

    #[test]
    fn the_shader_reads_the_parameters_it_declares_and_declares_the_entry_point() {
        assert!(SHADER.contains(&format!("fn {ENTRY}(")));
        for spec in &grade::PARAMS {
            assert!(
                SHADER.contains(&format!("params.{}", spec.id)),
                "the shader never reads params.{}",
                spec.id
            );
        }
        // The host owns every binding; a plugin that declared one of its own
        // would collide with the prelude.
        assert!(
            !SHADER.contains("@group("),
            "the shader binds resources itself"
        );
    }
}
