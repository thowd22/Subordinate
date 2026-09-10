//! The sandbox is closed until an install-time approval opens it.
//!
//! Three things are proved here against real directories on disk: a plugin
//! whose manifest never asked for `fs_read` cannot open a file that exists,
//! either through the host's own gate or through WASI (it gets no preopens at
//! all); the same plugin *can* once its manifest asks for the root and the user
//! approves it; and rewriting `plugin.toml` after that approval takes the grant
//! away again until it is re-approved (TASK-83, docs/PLAN.md §6.1).

use std::fs;
use std::path::{Path, PathBuf};

use sub_plugin::capability::{
    Access, ApprovalStatus, ApprovalStore, PathVars, ResolvedCapabilities,
};
use sub_plugin::manifest::Manifest;
use wasmtime_wasi::WasiCtxBuilder;

/// A manifest for one plugin id with the given `[capabilities]` body.
fn manifest(capabilities: &str) -> Manifest {
    Manifest::parse(&format!(
        "[plugin]\nid = \"com.example.reader\"\nname = \"Reader\"\nversion = \"0.1.0\"\napi = \"0.1\"\nworlds = [\"command\"]\n{capabilities}"
    ))
    .unwrap()
}

/// A uniquely named directory under the temp directory, removed by the OS.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sub-plugin-cap-{}-{name}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A project directory holding one readable file, plus a plugin data directory.
fn fixture() -> (PathBuf, PathBuf, PathBuf) {
    let root = temp_dir("fixture");
    let project = root.join("project");
    let data = root.join("data");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&data).unwrap();
    let secret = project.join("notes.txt");
    fs::write(&secret, b"timeline notes").unwrap();
    (root, project, secret)
}

#[test]
fn a_plugin_without_fs_read_cannot_open_files() {
    let (root, project, secret) = fixture();
    let vars = PathVars::new(&root.join("data"))
        .unwrap()
        .with_project(&project)
        .unwrap();

    // The file is really there: it is the capability, not the filesystem, that
    // refuses.
    assert_eq!(fs::read_to_string(&secret).unwrap(), "timeline notes");

    let manifest = manifest("");
    let mut store = ApprovalStore::new();
    store.approve(&manifest);
    let resolved = store.authorize(&manifest, &vars).unwrap();

    // The host gate refuses, with the stable code and the offending path.
    let denied = resolved.authorize_read(&secret).unwrap_err();
    assert_eq!(denied.code.as_str(), "plugin.capability_denied");
    assert_eq!(denied.details["capability"], "fs_read");
    assert_eq!(
        denied.details["path"],
        secret.display().to_string().as_str()
    );

    // And WASI is handed nothing to open: no preopens means the guest has no
    // directory handle to resolve any path against.
    assert!(resolved.preopens().is_empty());
    let mut builder = WasiCtxBuilder::new();
    resolved.apply_to_wasi(&mut builder).unwrap();
    let _ = builder.build();

    // Nor may it climb out of a root it was never given.
    assert!(
        resolved
            .authorize_read(&project.join("..").join("data"))
            .is_err()
    );
    assert!(resolved.authorize_network().is_err());
    assert!(resolved.authorize_shaders().is_err());
}

#[test]
fn approving_fs_read_opens_exactly_that_root() {
    let (root, project, secret) = fixture();
    let vars = PathVars::new(&root.join("data"))
        .unwrap()
        .with_project(&project)
        .unwrap();

    let manifest = manifest("[capabilities]\nfs_read = [\"$PROJECT\"]\n");
    let mut store = ApprovalStore::new();
    store.approve(&manifest);
    let resolved = store.authorize(&manifest, &vars).unwrap();

    resolved.authorize_read(&secret).unwrap();
    assert!(resolved.authorize_write(&secret).is_err());
    assert!(
        resolved
            .authorize_read(&root.join("data/cache.bin"))
            .is_err()
    );

    // The grant is a real WASI preopen of the project directory, read-only.
    assert_eq!(resolved.preopens().len(), 1);
    assert_eq!(resolved.preopens()[0].host_path(), project.as_path());
    assert_eq!(resolved.preopens()[0].guest_path(), "/project");
    assert_eq!(resolved.preopens()[0].access(), Access::ReadOnly);
    let mut builder = WasiCtxBuilder::new();
    resolved.apply_to_wasi(&mut builder).unwrap();
    let _ = builder.build();
}

#[test]
fn a_granted_root_that_is_gone_fails_to_preopen() {
    let root = temp_dir("missing");
    let vars = PathVars::new(&root)
        .unwrap()
        .with_project(&root.join("never-created"))
        .unwrap();
    let capabilities = manifest("[capabilities]\nfs_read = [\"$PROJECT\"]\n").capabilities;
    let resolved = ResolvedCapabilities::resolve(&capabilities, &vars).unwrap();

    let mut builder = WasiCtxBuilder::new();
    let err = resolved.apply_to_wasi(&mut builder).unwrap_err();
    assert_eq!(err.code.as_str(), "plugin.preopen_failed");
    assert_eq!(err.details["guest_path"], "/project");
}

#[test]
fn rewriting_the_manifest_takes_the_grant_away_until_it_is_approved_again() {
    let (root, project, secret) = fixture();
    let data = root.join("data");
    let vars = PathVars::new(&data)
        .unwrap()
        .with_project(&project)
        .unwrap();
    let approvals = root.join("state").join("approvals.json");

    // Install: the user approves a read of the project directory.
    let installed = manifest("[capabilities]\nfs_read = [\"$PROJECT\"]\n");
    let mut store = ApprovalStore::load(&approvals).unwrap();
    store.approve(&installed);
    store.save(&approvals).unwrap();

    // The record survives a restart, and the plugin loads on it.
    let store = ApprovalStore::load(&approvals).unwrap();
    assert!(store.status(&installed).is_approved());
    store
        .authorize(&installed, &vars)
        .unwrap()
        .authorize_read(&secret)
        .unwrap();

    // The plugin then rewrites its manifest to ask for writes and the network.
    let widened = manifest(
        "[capabilities]\nfs_read = [\"$PROJECT\"]\nfs_write = [\"$PROJECT\"]\nnetwork = true\n",
    );
    let ApprovalStatus::Changed {
        approved,
        capabilities_changed,
    } = store.status(&widened)
    else {
        panic!("a rewritten manifest must not stay approved");
    };
    assert!(capabilities_changed);
    assert!(approved.capabilities.fs_write.is_empty());

    let err = store.authorize(&widened, &vars).unwrap_err();
    assert_eq!(err.code.as_str(), "plugin.approval_stale");
    assert_eq!(err.details["capabilities_changed"], true);

    // Re-approving grants the wider set, and the write it now asks for works.
    let mut store = store;
    store.approve(&widened);
    store.save(&approvals).unwrap();
    let resolved = ApprovalStore::load(&approvals)
        .unwrap()
        .authorize(&widened, &vars)
        .unwrap();
    resolved.authorize_write(&secret).unwrap();
    resolved.authorize_network().unwrap();
    assert_eq!(resolved.preopens().len(), 1);
    assert_eq!(resolved.preopens()[0].access(), Access::ReadWrite);
}

#[test]
fn plugin_data_is_granted_without_an_open_project() {
    let root = temp_dir("data-only");
    let data = root.join("data");
    fs::create_dir_all(&data).unwrap();
    let vars = PathVars::new(&data).unwrap();

    let manifest = manifest("[capabilities]\nfs_write = [\"$PLUGIN_DATA/cache\"]\n");
    let mut store = ApprovalStore::new();
    store.approve(&manifest);
    let resolved = store.authorize(&manifest, &vars).unwrap();

    assert_eq!(resolved.preopens()[0].guest_path(), "/plugin-data/cache");
    assert_eq!(
        resolved.preopens()[0].host_path(),
        data.join("cache").as_path()
    );
    resolved
        .authorize_write(&data.join("cache").join("thumb.png"))
        .unwrap();
    assert!(resolved.authorize_write(&data.join("elsewhere")).is_err());

    // A $PROJECT root in the same manifest would refuse to resolve at all.
    let needs_project = manifest_needing_project();
    let mut store = ApprovalStore::new();
    store.approve(&needs_project);
    let err = store.authorize(&needs_project, &vars).unwrap_err();
    assert_eq!(err.code.as_str(), "plugin.unset_path_variable");
}

/// A manifest asking for the project directory.
fn manifest_needing_project() -> Manifest {
    manifest("[capabilities]\nfs_read = [\"$PROJECT\"]\n")
}

#[test]
fn a_manifest_read_from_disk_carries_its_own_approval() {
    let root = temp_dir("on-disk");
    let plugin_dir = root.join("plugin");
    fs::create_dir_all(&plugin_dir).unwrap();
    let path = plugin_dir.join("plugin.toml");
    fs::write(
        &path,
        "[plugin]\nid = \"com.example.reader\"\nname = \"Reader\"\nversion = \"0.1.0\"\napi = \"0.1\"\nworlds = [\"command\"]\n\n[capabilities]\nshaders = true\n",
    )
    .unwrap();

    let manifest = Manifest::read_dir(&plugin_dir).unwrap();
    let mut store = ApprovalStore::new();
    store.approve(&manifest);
    let vars = PathVars::new(&root.join("data")).unwrap();
    store
        .authorize(&manifest, &vars)
        .unwrap()
        .authorize_shaders()
        .unwrap();

    // A comment-only edit is not a change; a declared capability is.
    fs::write(
        &path,
        "# a plugin that tints\n[plugin]\nid = \"com.example.reader\"\nname = \"Reader\"\nversion = \"0.1.0\"\napi = \"0.1\"\nworlds = [\"command\"]\n\n[capabilities]\nshaders = true\n",
    )
    .unwrap();
    assert!(
        store
            .status(&Manifest::read_dir(&plugin_dir).unwrap())
            .is_approved()
    );

    fs::write(
        &path,
        "[plugin]\nid = \"com.example.reader\"\nname = \"Reader\"\nversion = \"0.1.0\"\napi = \"0.1\"\nworlds = [\"command\"]\n\n[capabilities]\nshaders = true\nnetwork = true\n",
    )
    .unwrap();
    assert!(
        !store
            .status(&Manifest::read_dir(&plugin_dir).unwrap())
            .is_approved()
    );
}

/// A stray absolute path in a manifest is not a way out of the sandbox.
#[test]
fn an_absolute_root_is_not_a_capability() {
    let vars = PathVars::new(Path::new("/data/plugins/x")).unwrap();
    let err = vars.expand("/etc/shadow", Access::ReadOnly).unwrap_err();
    assert_eq!(err.code.as_str(), "plugin.invalid_capability_path");
}
