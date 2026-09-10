//! Every plugin failure crosses its boundary as structured JSON: a stable
//! code, a message, the WIT item it belongs to when there is one, and a hint
//! (TASK-92, docs/PLAN.md §6.4).
//!
//! The unit tests in `src/errors.rs` cover the catalogue itself. These cover
//! the boundaries a real failure travels: a scan that meets a broken
//! `plugin.toml`, a reload that will not load, the `error` record a plugin
//! sees, and the copy of the table plugin authors read in the SDK.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sub_core::SubError;
use sub_plugin::dev::DevHost;
use sub_plugin::errors::{self, HINT, WIT};
use sub_plugin::registry::{PluginDirs, PluginRegistry};
use sub_plugin::{PluginId, WitError, codes};

/// A scratch user plugin directory.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("subordinate-plugin-errors-{name}"));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// Writes a plugin directory holding `manifest` as its `plugin.toml`.
fn plugin_dir(root: &Path, id: &str, manifest: &str) -> PathBuf {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).expect("a plugin directory");
    std::fs::write(dir.join("plugin.toml"), manifest).expect("a manifest");
    dir
}

/// A manifest the host accepts.
fn good(id: &str) -> String {
    format!(
        "[plugin]\nid = \"{id}\"\nname = \"One\"\nversion = \"0.1.0\"\n\
         api = \"0.1\"\nworlds = [\"command\"]\n"
    )
}

#[test]
fn a_broken_manifest_is_listed_with_its_code_and_a_hint() {
    let root = scratch("manifest");
    plugin_dir(
        &root,
        "com.example.broken",
        "[plugin]\nid = \"com.example.broken\"\n",
    );

    let scan = PluginRegistry::new(PluginDirs::new(&root))
        .scan()
        .expect("a scan survives a broken plugin");
    let failure = scan
        .failures()
        .first()
        .expect("the broken plugin is reported");

    assert!(
        failure.code.starts_with("plugin.invalid_manifest"),
        "{}",
        failure.code
    );
    let hint = failure.details[HINT].as_str().expect("a hint");
    assert!(hint.contains("plugin-manifest.json"), "{hint}");

    let json = serde_json::to_value(failure).expect("a JSON row");
    assert_eq!(json["details"][HINT], hint);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_failed_reload_reports_the_code_and_the_hint_the_loader_raised() {
    let root = scratch("reload");
    plugin_dir(&root, "com.example.one", &good("com.example.one"));

    let registry = Arc::new(PluginRegistry::new(PluginDirs::new(&root)));
    let mut host = DevHost::new(registry, |_, _| {
        Err(SubError::new(codes::LOAD_FAILED, "not a component"))
    });
    let id = PluginId::parse("com.example.one").expect("an id");

    let error = host.reload(&id).expect_err("the loader refuses it");
    assert_eq!(error.code, codes::LOAD_FAILED);
    let hint = error.details[HINT].as_str().expect("a hint");
    assert!(hint.contains("wasm32-wasip2"), "{hint}");

    let status = host.status(&id).expect("a recorded status");
    let recorded = status.error.as_ref().expect("the failure is recorded");
    assert_eq!(recorded.code, "plugin.load_failed");
    assert_eq!(recorded.details[HINT], hint);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn the_error_a_plugin_sees_names_the_wit_item_and_the_hint() {
    let error = SubError::new(codes::INVALID_AUDIO_BLOCK, "the block is the wrong length");
    let crossed = WitError::from(&error);

    assert_eq!(crossed.code, "plugin.invalid_audio_block");
    let detail = |key: &str| {
        crossed
            .details
            .iter()
            .find(|detail| detail.key == key)
            .map(|detail| detail.value.clone())
    };
    assert_eq!(
        detail(WIT).as_deref(),
        Some("\"subordinate:plugin/audio-effect.process\"")
    );
    assert!(detail(HINT).is_some_and(|hint| hint.contains("interleaved")));
}

#[test]
fn a_failure_that_never_reached_the_wit_boundary_still_carries_a_hint() {
    let error = errors::explain(SubError::new(
        codes::DEV_SOURCE_INVALID,
        "there is nothing to install there",
    ));
    assert!(!error.details.contains_key(WIT));
    assert!(
        error.details[HINT]
            .as_str()
            .is_some_and(|hint| { hint.contains("plugin.toml") })
    );
}

#[test]
fn the_sdk_publishes_the_same_catalogue() {
    let host = errors::CATALOGUE;
    let sdk = subordinate_sdk::errors::CATALOGUE;
    assert_eq!(host.len(), sdk.len(), "the two catalogues differ in size");
    for (host, sdk) in host.iter().zip(sdk) {
        assert_eq!(host.code, sdk.code);
        assert_eq!(
            host.wit, sdk.wit,
            "{} names a different WIT item",
            host.code
        );
        assert_eq!(host.hint, sdk.hint, "{}'s hint differs", host.code);
    }
}

#[test]
fn the_sdk_names_the_manifest_capability_and_reload_codes_apart() {
    use subordinate_sdk::errors::codes as sdk;

    // Distinct situations, distinct codes: an agent tells a manifest problem
    // from a capability problem from a failed reload without reading prose.
    for (code, expected) in [
        (sdk::INVALID_MANIFEST, codes::INVALID_MANIFEST),
        (sdk::MANIFEST_UNREADABLE, codes::MANIFEST_UNREADABLE),
        (sdk::CAPABILITY_DENIED, codes::CAPABILITY_DENIED),
        (sdk::NOT_APPROVED, codes::NOT_APPROVED),
        (sdk::APPROVAL_STALE, codes::APPROVAL_STALE),
        (sdk::LOAD_FAILED, codes::LOAD_FAILED),
        (sdk::LINK_FAILED, codes::LINK_FAILED),
    ] {
        assert_eq!(code, expected.as_str());
        assert!(
            subordinate_sdk::errors::info(code).is_some_and(|row| !row.hint.is_empty()),
            "{code} has no hint in the SDK"
        );
    }
}
