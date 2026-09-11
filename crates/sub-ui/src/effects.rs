//! What the inspector knows about the effects a user can apply.
//!
//! A project file stores a reference, never a declaration: a
//! [`ClipEffect`](sub_model::ClipEffect) names a plugin and binds the values
//! the user moved away from the plugin's defaults, and everything else — the
//! parameter list, the labels, the ranges, the WGSL — comes from the installed
//! plugin at load time (docs/PLAN.md §5.3, decision-6). The inspector
//! therefore needs a second source beside the project: this catalogue, which
//! is the editor's view of the installed `effect`-world plugins.
//!
//! It is built in two steps, because the two facts arrive at different times.
//! [`EffectCatalog::from_installed`] reads the install scan: every enabled
//! plugin whose manifest declares the `effect` world is something the user can
//! apply, and its manifest name is what the picker shows, whether or not the
//! host has instantiated it yet. [`EffectCatalog::declare`] then fills in the
//! parameters the plugin's `describe` handed back, lifted into
//! [`EffectParam`] by `sub-plugin`'s `render` feature. An effect with no
//! declaration yet is listed and applicable; it simply shows no controls until
//! its plugin has loaded.
//!
//! Nothing here holds a component, a wasmtime store or a GPU device, so the
//! panel can be painted in a test over a catalogue built by hand.
//!
//! ```
//! use sub_render::{EffectParam, ParamKind};
//! use sub_ui::effects::{EffectCatalog, EffectListing};
//!
//! let mut catalog = EffectCatalog::new();
//! catalog.push(EffectListing::new("com.example.grade", "Grade"));
//! catalog.declare(
//!     "com.example.grade",
//!     vec![EffectParam::new(
//!         "exposure",
//!         "Exposure",
//!         ParamKind::Float { min: -4.0, max: 4.0, default: 0.0, step: None },
//!     )],
//! );
//!
//! assert_eq!(catalog.name_of("com.example.grade"), "Grade");
//! assert_eq!(catalog.params_of("com.example.grade").len(), 1);
//! // A plugin that is not installed still has a name to paint: its id.
//! assert_eq!(catalog.name_of("com.example.gone"), "com.example.gone");
//! ```

use sub_plugin::manifest::World;
use sub_plugin::registry::InstalledPlugin;
use sub_render::EffectParam;

/// One installed effect plugin as the inspector sees it.
///
/// The plugin id is the identity a [`ClipEffect`](sub_model::ClipEffect)
/// carries, so it is what a command names; the name and the parameters are
/// only ever painted.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectListing {
    /// Reverse-DNS id of the plugin declaring the effect.
    pub plugin: String,
    /// The manifest's display name, which is what the picker shows.
    pub name: String,
    /// The parameters the plugin declared, in declaration order.
    ///
    /// Empty until the host has loaded the plugin and lifted its `describe`
    /// result, which is not a failure: the effect can still be applied and
    /// removed, it just offers no controls yet.
    pub params: Vec<EffectParam>,
}

impl EffectListing {
    /// An installed effect with no declaration read yet.
    #[must_use]
    pub fn new(plugin: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            name: name.into(),
            params: Vec::new(),
        }
    }

    /// The same listing carrying the parameters the plugin declared.
    #[must_use]
    pub fn with_params(mut self, params: Vec<EffectParam>) -> Self {
        self.params = params;
        self
    }
}

/// Every effect the user can apply, in the order the picker lists them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectCatalog {
    entries: Vec<EffectListing>,
}

impl EffectCatalog {
    /// An empty catalogue: no effect plugin is installed, or none has been
    /// scanned for yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// The catalogue of the enabled `effect`-world plugins in `installed`.
    ///
    /// A disabled plugin is left out — switching a plugin off is how a user
    /// stops it being offered — and a plugin declaring other worlds only is
    /// not an effect at all. Order follows the scan, which is sorted by id.
    #[must_use]
    pub fn from_installed(installed: &[InstalledPlugin]) -> Self {
        let entries = installed
            .iter()
            .filter(|plugin| {
                plugin.enabled && plugin.manifest.plugin.worlds.contains(&World::Effect)
            })
            .map(|plugin| EffectListing::new(plugin.id.as_str(), plugin.name()))
            .collect();
        Self { entries }
    }

    /// Adds one listing to the end of the catalogue.
    pub fn push(&mut self, listing: EffectListing) {
        self.entries.push(listing);
    }

    /// Records what `plugin` declared, once the host has loaded it.
    ///
    /// A plugin the catalogue does not hold — one loaded from a directory the
    /// scan has not caught up with — is added, so a declaration is never
    /// dropped on the floor.
    pub fn declare(&mut self, plugin: &str, params: Vec<EffectParam>) {
        if let Some(listing) = self
            .entries
            .iter_mut()
            .find(|listing| listing.plugin == plugin)
        {
            listing.params = params;
            return;
        }
        self.entries
            .push(EffectListing::new(plugin, plugin).with_params(params));
    }

    /// Every listing, in picker order.
    #[must_use]
    pub fn entries(&self) -> &[EffectListing] {
        &self.entries
    }

    /// Whether the catalogue offers nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The listing for `plugin`, if it is installed.
    #[must_use]
    pub fn listing(&self, plugin: &str) -> Option<&EffectListing> {
        self.entries.iter().find(|listing| listing.plugin == plugin)
    }

    /// What to call `plugin`: its display name, or its id when the plugin an
    /// applied effect names is not installed here.
    #[must_use]
    pub fn name_of<'a>(&'a self, plugin: &'a str) -> &'a str {
        self.listing(plugin)
            .map_or(plugin, |listing| listing.name.as_str())
    }

    /// The parameters `plugin` declared; empty when it is not installed or
    /// has not been loaded.
    #[must_use]
    pub fn params_of(&self, plugin: &str) -> &[EffectParam] {
        self.listing(plugin)
            .map_or(&[][..], |listing| listing.params.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::{EffectCatalog, EffectListing};
    use sub_plugin::manifest::Manifest;
    use sub_plugin::registry::{InstallLocation, InstalledPlugin};
    use sub_render::{EffectParam, ParamKind};

    /// An installed plugin declaring `worlds`.
    fn installed(id: &str, name: &str, worlds: &str, enabled: bool) -> InstalledPlugin {
        let manifest = Manifest::parse(&format!(
            "[plugin]\nid = \"{id}\"\nname = \"{name}\"\nversion = \"1.0.0\"\n\
             api = \"0.1\"\nworlds = [{worlds}]\n"
        ))
        .expect("a valid manifest");
        InstalledPlugin {
            id: manifest.plugin.id.clone(),
            location: InstallLocation::User,
            directory: format!("/plugins/{name}").into(),
            enabled,
            dev: false,
            manifest,
        }
    }

    #[test]
    fn only_enabled_effect_plugins_are_offered() {
        let catalog = EffectCatalog::from_installed(&[
            installed("com.example.grade", "Grade", "\"effect\"", true),
            installed("com.example.cutter", "Cutter", "\"command\"", true),
            installed("com.example.blur", "Blur", "\"effect\"", false),
        ]);
        assert_eq!(
            catalog
                .entries()
                .iter()
                .map(|listing| listing.plugin.as_str())
                .collect::<Vec<_>>(),
            vec!["com.example.grade"],
            "a command plugin is not an effect and a disabled one is not offered"
        );
        assert_eq!(catalog.name_of("com.example.grade"), "Grade");
    }

    #[test]
    fn a_listed_plugin_has_no_parameters_until_it_declares_them() {
        let mut catalog = EffectCatalog::new();
        catalog.push(EffectListing::new("com.example.grade", "Grade"));
        assert!(catalog.params_of("com.example.grade").is_empty());

        catalog.declare(
            "com.example.grade",
            vec![EffectParam::new(
                "exposure",
                "Exposure",
                ParamKind::Bool { default: true },
            )],
        );
        assert_eq!(catalog.params_of("com.example.grade").len(), 1);
        assert_eq!(
            catalog.entries().len(),
            1,
            "declaring fills the listing in rather than adding a second one"
        );
    }

    #[test]
    fn a_declaration_for_an_unlisted_plugin_is_kept() {
        let mut catalog = EffectCatalog::new();
        catalog.declare("com.example.blur", Vec::new());
        assert_eq!(catalog.entries().len(), 1);
        assert_eq!(
            catalog.name_of("com.example.blur"),
            "com.example.blur",
            "with its id standing in for a name"
        );
    }
}
