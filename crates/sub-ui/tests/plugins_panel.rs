//! The plugins panel on the shared `egui_kittest` harness.
//!
//! Two snapshots hold the list to what it should look like — a healthy pair of
//! installs, and the same list with a failed hot reload and a directory that
//! would not scan — and the interaction test drives Disable and Enable by
//! their accessibility labels over a real [`PluginRegistry`] in a temporary
//! directory, so the click really does reach `plugins.json` on disk.
//!
//! The snapshot rows are built by hand rather than scanned, because a scan's
//! rows carry the temporary directory they were found in and a path with a
//! timestamp in it cannot be committed as a picture.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use egui_kittest::kittest::Queryable;
use sub_plugin::dev::{ReloadError, ReloadStatus};
use sub_plugin::manifest::Manifest;
use sub_plugin::registry::{
    InstallLocation, InstalledPlugin, LoadFailure, PluginDirs, PluginRegistry,
};
use sub_ui::plugins_panel::{LoadStatus, PluginAction, PluginOutcome, PluginRow, PluginsPanel};

/// A manifest for `id`, declaring the worlds and capabilities a panel row has
/// to show.
fn manifest(id: &str, name: &str) -> Manifest {
    Manifest::parse(&format!(
        "[plugin]\nid = \"{id}\"\nname = \"{name}\"\nversion = \"0.3.1\"\n\
         api = \"0.1\"\nworlds = [\"command\", \"effect\"]\n\
         [capabilities]\nfs_read = [\"$PROJECT\"]\nshaders = true\n"
    ))
    .expect("a valid manifest")
}

/// One installed plugin at a fixed path, so a snapshot of it is stable.
fn installed(id: &str, name: &str, enabled: bool) -> InstalledPlugin {
    let manifest = manifest(id, name);
    InstalledPlugin {
        id: manifest.plugin.id.clone(),
        location: InstallLocation::User,
        directory: PathBuf::from("/plugins").join(id),
        enabled,
        dev: false,
        manifest,
    }
}

/// A successful load of `plugin`.
fn loaded(plugin: &InstalledPlugin, dev: bool) -> ReloadStatus {
    ReloadStatus {
        id: plugin.id.clone(),
        generation: 1,
        ok: true,
        dev,
        commands: 2,
        effects: 1,
        tools: 0,
        error: None,
    }
}

/// A hot reload of `plugin` that failed, as `--dev` installs report one.
fn failed(plugin: &InstalledPlugin) -> ReloadStatus {
    ReloadStatus {
        error: Some(ReloadError {
            code: "plugin.load_failed".to_owned(),
            message: "the component does not instantiate".to_owned(),
            details: BTreeMap::from([(
                "hint".to_owned(),
                serde_json::Value::String("rebuild the plugin and save again".to_owned()),
            )]),
        }),
        ..loaded(plugin, true)
    }
}

#[test]
fn the_installed_list_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let cutter = installed("com.example.cutter", "Silence cutter", true);
    let montage = installed("com.example.montage", "Montage", false);
    let rows = vec![
        PluginRow::new(&cutter, Some(&loaded(&cutter, false))),
        PluginRow::new(&montage, None),
    ];

    let mut panel = PluginsPanel::new();
    panel.set_rows(rows, Vec::new());
    assert!(!panel.has_problems(), "this list is the healthy one");

    let mut harness = support::panel_harness(|ui| {
        panel.ui(ui);
    });
    harness.run();
    support::snapshot(&mut harness, "plugins_panel_installed");
}

#[test]
fn the_error_list_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let cutter = installed("com.example.cutter", "Silence cutter", true);
    let rows = vec![PluginRow::new(&cutter, Some(&failed(&cutter)))];
    let failures = vec![LoadFailure {
        directory: PathBuf::from("/plugins/broken"),
        location: InstallLocation::User,
        code: "plugin.invalid_manifest".to_owned(),
        message: "the manifest is not valid TOML".to_owned(),
        details: BTreeMap::from([(
            "field".to_owned(),
            serde_json::Value::String("plugin.version".to_owned()),
        )]),
    }];

    let mut panel = PluginsPanel::new();
    panel.set_rows(rows, failures);
    assert!(panel.has_problems());

    let mut harness = support::panel_harness(|ui| {
        panel.ui(ui);
    });
    harness.run();
    support::snapshot(&mut harness, "plugins_panel_errors");
}

/// A uniquely named directory under the temp directory, removed at the end of
/// the test that made it.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sub-ui-plugins-{}-{name}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("the temp directory is writable");
    dir
}

/// Writes a plugin directory holding `manifest`, and returns the user
/// directory it lives in.
fn install_into(user: &Path, id: &str, name: &str) {
    let directory = user.join(id);
    std::fs::create_dir_all(&directory).expect("the plugin directory is writable");
    std::fs::write(
        directory.join("plugin.toml"),
        format!(
            "[plugin]\nid = \"{id}\"\nname = \"{name}\"\nversion = \"0.3.1\"\n\
             api = \"0.1\"\nworlds = [\"command\"]\n"
        ),
    )
    .expect("the manifest is writable");
}

#[test]
fn clicking_disable_and_enable_switches_the_plugin_in_the_registry() {
    let user = temp_dir("switch");
    install_into(&user, "com.example.cutter", "Silence cutter");
    let registry = PluginRegistry::new(PluginDirs::new(&user));

    let mut panel = PluginsPanel::new();
    panel.sync(&registry.scan().expect("the scan walks the temp dir"), &[]);
    assert_eq!(panel.rows().len(), 1);
    assert!(panel.rows()[0].enabled, "a fresh install is switched on");
    assert_eq!(panel.rows()[0].status, LoadStatus::NotLoaded);

    // The panel raises actions and performs none of them: the harness applies
    // what it raised through the registry, exactly as the app does.
    let mut harness = support::panel_harness_state(
        (panel, registry, Vec::<PluginOutcome>::new()),
        |ui, (panel, registry, outcomes)| {
            if let Some(action) = panel.ui(ui) {
                let outcome = action.perform(registry).expect("the action applies");
                outcomes.push(outcome);
                panel.sync(&registry.scan().expect("the rescan works"), &[]);
            }
        },
    );
    harness.run();

    harness.get_by_label("Disable").click();
    harness.run();
    {
        let (panel, registry, outcomes) = harness.state();
        assert_eq!(outcomes.len(), 1, "one action was performed");
        assert!(
            matches!(&outcomes[0], PluginOutcome::Switched(change) if !change.enabled && change.changed),
            "the click switched the plugin off: {:?}",
            outcomes[0]
        );
        assert!(
            !panel.rows()[0].enabled,
            "the rescanned row is switched off"
        );
        assert_eq!(panel.rows()[0].status, LoadStatus::Disabled);
        let scan = registry.scan().expect("the scan works");
        assert_eq!(scan.enabled().count(), 0, "nothing is enabled on disk");
    }

    // The button is now the other one, and it switches the plugin back on.
    harness.get_by_label("Enable").click();
    harness.run();
    {
        let (panel, registry, outcomes) = harness.state();
        assert_eq!(outcomes.len(), 2);
        assert!(panel.rows()[0].enabled, "the plugin is switched back on");
        assert_eq!(
            registry.scan().expect("the scan works").enabled().count(),
            1
        );
    }

    std::fs::remove_dir_all(&user).ok();
}

#[test]
fn removing_a_plugin_deletes_its_directory() {
    let user = temp_dir("remove");
    install_into(&user, "com.example.cutter", "Silence cutter");
    let registry = PluginRegistry::new(PluginDirs::new(&user));
    let scan = registry.scan().expect("the scan works");
    let id = scan.plugins()[0].id.clone();
    let directory = scan.plugins()[0].directory.clone();

    let outcome = PluginAction::Remove(id)
        .perform(&registry)
        .expect("the removal applies");
    assert!(matches!(outcome, PluginOutcome::Removed(_)));
    assert!(!directory.exists(), "the plugin directory is gone");
    assert!(
        registry
            .scan()
            .expect("the scan works")
            .plugins()
            .is_empty()
    );

    std::fs::remove_dir_all(&user).ok();
}

#[test]
fn an_action_naming_a_plugin_that_is_gone_is_refused_with_its_code() {
    let user = temp_dir("gone");
    let registry = PluginRegistry::new(PluginDirs::new(&user));
    let id = manifest("com.example.ghost", "Ghost").plugin.id.clone();

    let error = PluginAction::Disable(id)
        .perform(&registry)
        .expect_err("nothing is installed under that id");
    assert_eq!(error.code.as_str(), "plugin.not_installed");

    std::fs::remove_dir_all(&user).ok();
}

#[test]
fn clicking_open_folder_raises_the_plugins_own_directory() {
    // The action is raised, not performed: starting a file manager is a
    // platform call with nothing to assert on in a headless test.
    let cutter = installed("com.example.cutter", "Silence cutter", true);
    let mut panel = PluginsPanel::new();
    panel.set_rows(vec![PluginRow::new(&cutter, None)], Vec::new());

    let mut harness = support::panel_harness_state(
        (panel, Vec::<PluginAction>::new()),
        |ui, (panel, raised)| {
            if let Some(action) = panel.ui(ui) {
                raised.push(action);
            }
        },
    );
    harness.run();
    harness.get_by_label("Open folder").click();
    harness.run();

    let (_, raised) = harness.state();
    assert_eq!(
        raised,
        &vec![PluginAction::OpenFolder(cutter.directory.clone())]
    );
}
