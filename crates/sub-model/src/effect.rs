//! The effect stack a clip carries: which plugin effects run on it, in which
//! order, with which parameter values.
//!
//! A plugin effect is a *declaration* — parameters plus one WGSL shader — that
//! the plugin host lifts out of the component and the compositor compiles
//! (docs/PLAN.md §5.3, decision-6). Nothing of that belongs in a project file:
//! what a project stores is the reference, which is exactly what this module
//! models. A [`ClipEffect`] names the plugin whose `effect` world declared the
//! shader and binds a value to the parameters the user moved away from their
//! declared defaults; everything else comes from the plugin at load time.
//!
//! That split is what keeps a project file readable by a build with no plugins
//! installed, and what lets `sub-render`'s
//! [`EffectSource`](https://docs.rs/sub-render) stay ignorant of the model: the
//! host resolves a [`ClipEffect`] against the installed plugin and hands the
//! compositor a bound instance.
//!
//! Values are [`Fixed6`], never floats, for the same reason every other clip
//! parameter is (see [`crate::params`]): a project saves byte-identically on
//! every platform and a clip stays `Eq` and `Hash`.
//!
//! ```
//! use sub_model::effect::{ClipEffect, EffectValue};
//! use sub_model::params::Fixed6;
//!
//! let grade = ClipEffect::new("com.subordinate.color")
//!     .unwrap()
//!     .with_param("exposure", EffectValue::Float(Fixed6::ONE));
//! assert!(grade.enabled);
//! assert_eq!(grade.param("exposure"), Some(EffectValue::Float(Fixed6::ONE)));
//! grade.validate().unwrap();
//! ```

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};

use crate::codes;
use crate::ids::EffectId;
use crate::params::Fixed6;

/// The longest plugin id a project file may name.
///
/// The same bound `sub-plugin`'s `PluginId` enforces, repeated here because
/// the model may not depend on the plugin host: a project file is validated by
/// whoever opens it, including a build that installs no plugins at all.
const MAX_PLUGIN_ID: usize = 128;

/// A value bound to one declared effect parameter.
///
/// The cases mirror the WIT `param-value` variant one for one, with `Fixed6`
/// wherever WIT has an `f32`, so a value survives save, load and undo exactly
/// as it was set. `sub-plugin` converts between the two vocabularies.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EffectValue {
    /// A continuous value, e.g. an exposure in stops.
    Float(Fixed6),
    /// A whole number, e.g. a sample count.
    Int(i32),
    /// A flag.
    Bool(bool),
    /// A linear RGBA colour, one channel per element.
    Color([Fixed6; 4]),
    /// Index into the variants the plugin declared.
    Choice(u32),
}

/// One plugin effect applied to a clip.
///
/// Order in [`Clip::effects`](crate::Clip::effects) is the order the
/// compositor runs them in, first applied first.
///
/// A plugin targeting the `effect` world declares exactly one effect, so the
/// plugin id is the whole reference; [`params`](ClipEffect::params) holds only
/// the values that differ from the declared defaults, keyed by the parameter
/// ids the plugin declared. A value for a parameter the installed plugin no
/// longer declares is kept in the file and ignored when binding, so a project
/// that outlives an edit to a plugin still opens and still renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipEffect {
    /// Stable identity, preserved across save, load and undo.
    pub id: EffectId,
    /// Reverse-DNS id of the plugin that declares the effect, e.g.
    /// `com.subordinate.color`.
    pub plugin: String,
    /// Whether the effect runs. A disabled effect stays in the stack with its
    /// values so it can be switched back on.
    pub enabled: bool,
    /// Values bound by parameter id; anything absent takes the plugin's
    /// declared default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, EffectValue>,
}

impl ClipEffect {
    /// Creates an enabled effect referring to `plugin`, with every parameter
    /// left at the plugin's declared default.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_effect` when `plugin` is not a reverse-DNS
    /// plugin id.
    pub fn new(plugin: impl Into<String>) -> SubResult<Self> {
        let plugin = plugin.into();
        check_plugin_id(&plugin)?;
        Ok(Self {
            id: EffectId::new(),
            plugin,
            enabled: true,
            params: BTreeMap::new(),
        })
    }

    /// Binds `value` to the parameter `id`.
    #[must_use]
    pub fn with_param(mut self, id: impl Into<String>, value: EffectValue) -> Self {
        self.params.insert(id.into(), value);
        self
    }

    /// The value bound to `id`, if the effect binds one.
    #[must_use]
    pub fn param(&self, id: &str) -> Option<EffectValue> {
        self.params.get(id).copied()
    }

    /// Checks the effect's invariants: a well-formed plugin id and well-formed
    /// parameter ids.
    ///
    /// # Errors
    ///
    /// Returns `model.invalid_effect` describing the first broken invariant.
    pub fn validate(&self) -> SubResult<()> {
        check_plugin_id(&self.plugin)?;
        for id in self.params.keys() {
            if !is_param_id(id) {
                let message = "effect parameter id must start with a lower-case ascii letter and \
                               carry only lower-case letters, digits and underscores";
                return Err(invalid(message)
                    .with_detail("plugin", self.plugin.clone())
                    .with_detail("param", id.clone()));
            }
        }
        Ok(())
    }
}

/// A `model.invalid_effect` error.
fn invalid(message: &str) -> SubError {
    SubError::new(codes::INVALID_EFFECT, message)
}

/// Checks a reverse-DNS plugin id: two or more dot-separated segments of
/// `[a-z][a-z0-9-]*` that do not end in a hyphen.
fn check_plugin_id(plugin: &str) -> SubResult<()> {
    let well_formed = plugin.len() <= MAX_PLUGIN_ID
        && plugin.split('.').count() >= 2
        && plugin.split('.').all(is_plugin_segment);
    if well_formed {
        return Ok(());
    }
    Err(
        invalid("effect plugin must be a reverse-DNS plugin id, e.g. \"com.example.grade\"")
            .with_detail("plugin", plugin.to_owned())
            .with_detail("max_length", MAX_PLUGIN_ID),
    )
}

/// One segment of a reverse-DNS plugin id.
fn is_plugin_segment(segment: &str) -> bool {
    let mut characters = segment.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && characters.clone().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        && !segment.ends_with('-')
}

/// A parameter id as the plugin declares it: the same shape `sub-render`
/// accepts, because it is the name of a member of the uniform block.
fn is_param_id(id: &str) -> bool {
    let mut characters = id.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_effect_is_enabled_and_binds_nothing() {
        let effect = ClipEffect::new("com.subordinate.color").expect("a valid plugin id");
        assert!(effect.enabled);
        assert!(effect.params.is_empty());
        effect.validate().expect("a fresh effect is valid");
    }

    #[test]
    fn a_plugin_id_that_is_not_reverse_dns_is_refused() {
        for plugin in ["color", "Com.Subordinate.Color", "com..color", "com.color-"] {
            let error = ClipEffect::new(plugin).expect_err("refused");
            assert_eq!(error.code, codes::INVALID_EFFECT, "{plugin}");
        }
    }

    #[test]
    fn a_plugin_id_longer_than_the_bound_is_refused() {
        let long = format!("com.{}", "a".repeat(MAX_PLUGIN_ID));
        let error = ClipEffect::new(long).expect_err("refused");
        assert_eq!(error.code, codes::INVALID_EFFECT);
    }

    #[test]
    fn a_parameter_id_the_shader_could_not_name_is_refused() {
        let effect = ClipEffect::new("com.subordinate.color")
            .expect("a valid plugin id")
            .with_param("Exposure", EffectValue::Float(Fixed6::ONE));
        let error = effect.validate().expect_err("refused");
        assert_eq!(error.code, codes::INVALID_EFFECT);
    }

    #[test]
    fn values_round_trip_through_json_as_integers() {
        let effect = ClipEffect::new("com.subordinate.color")
            .expect("a valid plugin id")
            .with_param("exposure", EffectValue::Float(Fixed6::ONE))
            .with_param(
                "tint",
                EffectValue::Color([Fixed6::ONE, Fixed6::ZERO, Fixed6::ZERO, Fixed6::ONE]),
            );
        let text = serde_json::to_string(&effect).expect("serialises");
        assert!(text.contains("\"float\":1000000"), "{text}");
        assert_eq!(
            serde_json::from_str::<ClipEffect>(&text).expect("deserialises"),
            effect
        );
    }
}
