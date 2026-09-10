//! The manifests the first-party plugins ship are ones this host accepts.
//!
//! A `plugin.toml` is the only thing the host reads from an uninstalled plugin
//! (docs/PLAN.md §6.3), and it is checked in rather than generated, so nothing
//! else in the build would notice if one drifted out of what
//! [`Manifest::parse`] takes — a misspelt capability or a world this API
//! version no longer names would only show up at install time. These plugins
//! live in their own cargo workspaces, so this is the one test that sees them.

use std::path::{Path, PathBuf};

use sub_plugin::manifest::{HOST_API_VERSION, MANIFEST_FILE_NAME, Manifest, World};

/// The repository root, from this crate's own directory.
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/sub-plugin is two levels below the repository root")
        .to_path_buf()
}

/// Parses the manifest of the plugin whose crate directory is `crate_dir`.
fn manifest(crate_dir: &str) -> Manifest {
    let path = repository_root().join(crate_dir).join(MANIFEST_FILE_NAME);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
    Manifest::parse(&text).unwrap_or_else(|error| {
        panic!(
            "{} is a manifest this host accepts: {error}",
            path.display()
        )
    })
}

#[test]
fn the_otio_importer_asks_only_to_read_the_project_folder() {
    let manifest = manifest("plugins/otio/otio-importer");
    assert_eq!(manifest.plugin.id.as_str(), "com.subordinate.otio-import");
    assert!(manifest.plugin.api.is_supported_by_host());
    assert!(manifest.declares(World::Importer));
    assert_eq!(manifest.capabilities.fs_read, ["$PROJECT"]);
    assert!(manifest.capabilities.fs_write.is_empty());
    assert!(!manifest.capabilities.network);
    assert!(!manifest.capabilities.shaders);
}

#[test]
fn the_otio_exporter_is_a_command_plugin_that_writes_the_project_folder() {
    let manifest = manifest("plugins/otio/otio-exporter");
    assert_eq!(manifest.plugin.id.as_str(), "com.subordinate.otio-export");
    assert!(manifest.plugin.api.is_supported_by_host());
    // Not `exporter`: that world contributes encoder presets, and writing an
    // OTIO document encodes nothing.
    assert!(manifest.declares(World::Command));
    assert!(!manifest.declares(World::Exporter));
    assert_eq!(manifest.capabilities.fs_write, ["$PROJECT"]);
    assert!(!manifest.capabilities.network);
}

#[test]
fn both_are_built_against_the_interface_version_this_host_implements() {
    for crate_dir in ["plugins/otio/otio-importer", "plugins/otio/otio-exporter"] {
        let manifest = manifest(crate_dir);
        assert_eq!(manifest.plugin.api.major, HOST_API_VERSION.major);
        assert!(manifest.plugin.api.minor <= HOST_API_VERSION.minor);
    }
}
