//! Plugin-contributed commands against a real engine.
//!
//! The registry is plain data, but the promise that matters to a user is a
//! behaviour: however many primitives a plugin command applies through the
//! Command API, one run is one step on the undo stack, and a run that fails
//! leaves the project untouched. That is asserted here against a real
//! [`sub_edit::Engine`] rather than a stand-in, because the grouping lives in
//! the engine's history and nowhere else.

use sub_core::{ErrorCode, SubError};
use sub_edit::commands::{AddTrack, RenameTrack};
use sub_edit::{Engine, EngineHandle};
use sub_model::sequence::SequenceSettings;
use sub_model::{Project, Sequence, SequenceId, TrackKind};
use sub_plugin::WitError;
use sub_plugin::menu::{CommandDesc, PluginCommandRegistry, run_as_undo_group};

/// A description as a plugin would declare it.
fn desc(id: &str, title: &str, shortcut: Option<&str>) -> CommandDesc {
    CommandDesc {
        id: id.to_owned(),
        title: title.to_owned(),
        shortcut: shortcut.map(str::to_owned),
    }
}

/// An engine over a project with one empty sequence.
fn engine() -> (Engine, SequenceId) {
    let mut project = Project::new("plugin commands");
    let sequence = Sequence::new("Main", SequenceSettings::default());
    let id = sequence.id;
    project.sequences.push(sequence);
    (Engine::spawn(project).expect("the engine starts"), id)
}

/// Two edits, the shape a plugin command has: several primitives, each one an
/// ordinary undoable command applied through the handle.
fn two_edits(handle: &EngineHandle, sequence: SequenceId) -> Result<(), WitError> {
    handle
        .apply(AddTrack::new(sequence, "V1", TrackKind::Video))
        .map_err(WitError::from)?;
    let track = handle.snapshot().sequences[0].tracks[0].id;
    handle
        .apply(RenameTrack::new(sequence, track, "Picture"))
        .map_err(WitError::from)?;
    Ok(())
}

#[test]
fn one_plugin_command_run_is_one_undo_step() {
    let (engine, sequence) = engine();
    let handle = engine.handle().clone();
    let mut registry = PluginCommandRegistry::new();
    registry
        .register(
            "com.example.tracks",
            &[desc("setup", "Set up tracks", None)],
        )
        .expect("the description is valid");
    let command = registry.require("com.example.tracks/setup").unwrap();

    run_as_undo_group::<_, WitError>(&handle, command, || two_edits(&handle, sequence))
        .expect("the run succeeds");

    let history = handle.history().expect("history");
    assert!(!history.in_group, "the group is closed again");
    assert_eq!(
        history.undo_len, 1,
        "two primitives collapsed into one undo step"
    );
    assert_eq!(
        history.undo_label.as_deref(),
        Some("Set up tracks"),
        "the step is labelled with the command's title"
    );
    assert_eq!(handle.snapshot().sequences[0].tracks[0].name, "Picture");

    // One undo takes the whole plugin command back.
    handle.undo().expect("undo").expect("there was a step");
    assert!(handle.snapshot().sequences[0].tracks.is_empty());
    let history = handle.history().expect("history");
    assert_eq!(history.undo_len, 0);
    assert_eq!(history.redo_len, 1);
}

#[test]
fn a_failed_run_rolls_its_group_back_and_keeps_its_own_error() {
    let (engine, sequence) = engine();
    let handle = engine.handle().clone();
    let mut registry = PluginCommandRegistry::new();
    registry
        .register(
            "com.example.tracks",
            &[desc("setup", "Set up tracks", None)],
        )
        .unwrap();
    let command = registry.require("com.example.tracks/setup").unwrap();

    let error = run_as_undo_group::<(), WitError>(&handle, command, || {
        two_edits(&handle, sequence)?;
        Err(WitError::from(SubError::new(
            ErrorCode::from_static("plugin.command_rejected"),
            "the plugin gave up",
        )))
    })
    .expect_err("the run failed");

    // The plugin's own error survives the rollback.
    assert_eq!(error.code, "plugin.command_rejected");

    let history = handle.history().expect("history");
    assert!(!history.in_group, "the failed group is closed");
    assert_eq!(history.undo_len, 0, "nothing is left on the undo stack");
    assert!(
        handle.snapshot().sequences[0].tracks.is_empty(),
        "the edits the run applied were undone"
    );
}

#[test]
fn the_registry_answers_the_menu_and_the_shortcut_registry() {
    let mut registry = PluginCommandRegistry::new();
    registry
        .register(
            "com.example.silence-cutter",
            &[
                desc("cut-silence", "Cut silence", Some("Ctrl+Shift+K")),
                desc("report", "Silence report", None),
            ],
        )
        .unwrap();
    registry
        .register(
            "com.example.montage",
            &[desc("build", "Build montage", None)],
        )
        .unwrap();

    // Menu order is registration order, then declaration order.
    let titles: Vec<&str> = registry
        .commands()
        .iter()
        .map(sub_plugin::PluginCommand::title)
        .collect();
    assert_eq!(titles, ["Cut silence", "Silence report", "Build montage"]);
    assert_eq!(
        registry.plugins(),
        ["com.example.silence-cutter", "com.example.montage"]
    );

    // Only the command that asked for a chord offers one to the registry.
    let chords: Vec<Option<&str>> = registry
        .commands()
        .iter()
        .map(sub_plugin::PluginCommand::shortcut)
        .collect();
    assert_eq!(chords, [Some("Ctrl+Shift+K"), None, None]);
}
