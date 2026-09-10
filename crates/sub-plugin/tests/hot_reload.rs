//! The developer loop end to end: a dev install, a rebuild, a reload.
//!
//! The unit tests in `sub_plugin::dev` prove the pieces — the watcher settles
//! before it reports, a reload swaps registrations, a failed reload keeps the
//! previous version. What is asserted here is the promise the loop makes to
//! the agent writing a plugin (docs/PLAN.md §6.4): rebuild the component and
//! the running host has picked it up, with its commands, effects and tools
//! re-registered, **within a second** and with nothing else disturbed.
//!
//! The loader is the test's own — instantiating a real component needs the
//! host's state, which is the editor's business, not this crate's — and it
//! answers what the component's bytes say, so a rebuild is observable as a
//! different registration.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sub_edit::Engine;
use sub_edit::commands::AddTrack;
use sub_model::sequence::SequenceSettings;
use sub_model::{Project, Sequence, TrackKind};
use sub_plugin::dev::{DevHost, PluginArtifacts, WASM_FILE_NAME, install, watch_and_reload};
use sub_plugin::menu::CommandDesc;
use sub_plugin::registry::{InstalledPlugin, PluginDirs, PluginRegistry};

/// A core module header. Nothing compiles it: the loader here is the test's.
const WASM: &[u8] = b"\0asm\x01\0\0\0";

/// How long a test waits for the watcher before giving up. The loop promises a
/// second; the assertion is what proves it.
const WITHIN: Duration = Duration::from_secs(1);

/// A source tree and an empty user plugin directory under one scratch root.
fn scratch(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("subordinate-hot-reload-{name}"));
    std::fs::remove_dir_all(&root).ok();
    let source = root.join("source");
    std::fs::create_dir_all(&source).expect("a source tree");
    std::fs::write(
        source.join("plugin.toml"),
        "[plugin]\nid = \"com.example.one\"\nname = \"One\"\nversion = \"0.1.0\"\n\
         api = \"0.1\"\nworlds = [\"command\"]\n",
    )
    .expect("a manifest");
    std::fs::write(source.join("plugin.wasm"), WASM).expect("a built component");
    (source, root.join("user"))
}

/// A loader that reads the component and contributes one command per byte past
/// the header, so a rebuild changes what is registered.
fn artifacts(path: &Path) -> PluginArtifacts {
    let bytes = std::fs::read(path).expect("the installed component");
    PluginArtifacts {
        commands: (WASM.len()..=bytes.len())
            .map(|index| CommandDesc {
                id: format!("do_{index}"),
                title: format!("Do {index}"),
                shortcut: None,
            })
            .collect(),
        ..PluginArtifacts::default()
    }
}

/// Waits for `check` to hold, up to [`WITHIN`], and answers how long it took.
fn wait_for(mut check: impl FnMut() -> bool) -> Duration {
    let started = Instant::now();
    while started.elapsed() < WITHIN {
        if check() {
            return started.elapsed();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the change was not picked up within {WITHIN:?}");
}

#[test]
fn a_rebuild_of_a_dev_installed_plugin_is_reloaded_within_a_second() {
    let (source, user) = scratch("rebuild");
    let installed = install(&PluginDirs::new(&user), None, &source, true).expect("a dev install");
    assert!(
        installed.directory.join(WASM_FILE_NAME).exists(),
        "the built component is installed under the name the host loads"
    );

    let registry = Arc::new(PluginRegistry::new(PluginDirs::new(&user)));
    let host = Arc::new(Mutex::new(DevHost::new(
        registry,
        |_: &InstalledPlugin, path: &Path| Ok(artifacts(path)),
    )));
    host.lock()
        .expect("the host")
        .reload(&installed.id)
        .expect("the first load");
    assert_eq!(host.lock().expect("the host").commands().len(), 1);

    // The watcher is what the editor and `serve` run.
    let watching = watch_and_reload(Arc::clone(&host), Duration::from_millis(50));

    // The agent rebuilds: the component grows by two bytes.
    std::fs::write(&installed.source.wasm, b"\0asm\x01\0\0\0\0\0").expect("a rebuild");
    let took = wait_for(|| {
        host.lock()
            .expect("the host")
            .status(&installed.id)
            .is_some_and(|status| status.commands == 3)
    });
    assert!(took < WITHIN, "the reload took {took:?}");

    let host = host.lock().expect("the host");
    let status = host.status(&installed.id).expect("a status");
    assert!(status.ok);
    assert!(status.dev, "it is recorded as a dev install");
    assert!(
        status.generation >= 2,
        "the rebuild is a further load, not the first"
    );
    assert_eq!(status.commands, 3, "the rebuilt version registers three");
    assert_eq!(
        host.commands().len(),
        3,
        "the previous registration was replaced, not added to"
    );
    assert!(host.commands().get("com.example.one/do_10").is_some());
    drop(watching);

    std::fs::remove_dir_all(source.parent().expect("the scratch root")).ok();
}

#[test]
fn a_rebuild_that_does_not_load_leaves_the_running_version_in_place() {
    let (source, user) = scratch("broken");
    let installed = install(&PluginDirs::new(&user), None, &source, true).expect("a dev install");
    let registry = Arc::new(PluginRegistry::new(PluginDirs::new(&user)));
    let host = Arc::new(Mutex::new(DevHost::new(
        registry,
        |_: &InstalledPlugin, path: &Path| {
            let bytes = std::fs::read(path).expect("the installed component");
            if bytes.starts_with(WASM) {
                Ok(artifacts(path))
            } else {
                Err(sub_core::SubError::new(
                    sub_plugin::codes::LOAD_FAILED,
                    "not a component",
                ))
            }
        },
    )));
    host.lock()
        .expect("the host")
        .reload(&installed.id)
        .expect("the first load");
    let watching = watch_and_reload(Arc::clone(&host), Duration::from_millis(50));

    // A broken build lands where the good one was.
    std::fs::write(&installed.source.wasm, b"not wasm at all").expect("a broken build");
    wait_for(|| {
        host.lock()
            .expect("the host")
            .status(&installed.id)
            .is_some_and(|status| !status.ok)
    });

    let host = host.lock().expect("the host");
    let status = host.status(&installed.id).expect("a status");
    assert_eq!(
        status.commands, 1,
        "the counts still describe the version that is loaded"
    );
    assert_eq!(
        status.error.as_ref().map(|error| error.code.as_str()),
        Some("plugin.load_failed"),
        "the failure is recorded for the panel to show",
    );
    assert_eq!(
        host.commands().len(),
        1,
        "the version that was running is still registered"
    );
    drop(watching);

    std::fs::remove_dir_all(source.parent().expect("the scratch root")).ok();
}

#[test]
fn a_reload_leaves_the_engine_and_its_undo_stack_alone() {
    let (source, user) = scratch("engine");
    let installed = install(&PluginDirs::new(&user), None, &source, true).expect("a dev install");

    // A real engine, mid-edit: one track added, so there is a step to lose.
    let mut project = Project::new("hot reload");
    let sequence = Sequence::new("Main", SequenceSettings::default());
    let sequence_id = sequence.id;
    project.sequences.push(sequence);
    let engine = Engine::spawn(project).expect("the engine starts");
    let handle = engine.handle().clone();
    handle
        .apply(AddTrack::new(sequence_id, "V1", TrackKind::Video))
        .expect("a track");
    let before = handle.snapshot();
    let history_before = handle.history().expect("history");

    let registry = Arc::new(PluginRegistry::new(PluginDirs::new(&user)));
    let mut host = DevHost::new(registry, |_: &InstalledPlugin, path: &Path| {
        Ok(artifacts(path))
    });
    host.reload(&installed.id).expect("the first load");
    std::fs::write(&installed.source.wasm, b"\0asm\x01\0\0\0\0\0").expect("a rebuild");
    host.reload(&installed.id).expect("the reload");

    let after = handle.snapshot();
    let history_after = handle.history().expect("history");
    assert_eq!(after.sequences[0].tracks.len(), 1);
    assert_eq!(
        after.sequences[0].tracks[0].name,
        before.sequences[0].tracks[0].name
    );
    assert_eq!(history_after.undo_len, history_before.undo_len);
    assert_eq!(history_after.redo_len, history_before.redo_len);
    assert_eq!(history_after.undo_label, history_before.undo_label);

    // And the edit still undoes to exactly where it was.
    handle.undo().expect("undo").expect("there was a step");
    assert!(handle.snapshot().sequences[0].tracks.is_empty());
    engine.shutdown().expect("the engine stops");
    std::fs::remove_dir_all(source.parent().expect("the scratch root")).ok();
}
