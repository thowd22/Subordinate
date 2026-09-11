//! The headless plugin test harness, end to end (TASK-91).
//!
//! Real components, a real engine and the real Command API dispatcher: the
//! `editor` guest edits the fixture project through `run-command`, and the
//! `tinter` guest declares WGSL the harness compiles and renders a frame with.
//! What is under test is the contract `subordinate-cli plugin test` publishes
//! — a structured report whose checks say what worked, an `ok` that is false
//! the moment one does not, and a project whose state after a run is both
//! reported and undone.
//!
//! The guests are built by this crate's build script for `wasm32-wasip2`;
//! where that target is not installed the whole file compiles out rather than
//! failing the build.

#![cfg(not(no_wasm_guests))]

use std::path::Path;
use std::sync::Arc;

use sub_command::Dispatcher;
use sub_edit::Engine;
use sub_edit::commands::{AddTrack, CreateSequence};
use sub_model::{Project, SequenceSettings, TrackKind};
use sub_plugin::TimelineExpectation;
use sub_plugin::expect::{ClipExpectation, SequenceExpectation, TrackExpectation};
use sub_plugin::harness::{CheckStatus, Harness, TestReport};
use sub_plugin::manifest::{Manifest, PluginId};

/// The guest components the build script produced.
mod guests {
    use std::path::Path;

    /// The plugin that adds a track through the Command API.
    pub fn editor() -> &'static Path {
        Path::new(env!("SUB_PLUGIN_GUEST_EDITOR"))
    }

    /// The plugin that declares a shader.
    pub fn tinter() -> &'static Path {
        Path::new(env!("SUB_PLUGIN_GUEST_TINTER"))
    }

    /// The plugin that never returns, for the failure path.
    pub fn looper() -> &'static Path {
        Path::new(env!("SUB_PLUGIN_GUEST_LOOPER"))
    }
}

/// A manifest declaring one world, as an installed plugin's would.
fn manifest(id: &str, world: &str) -> Manifest {
    Manifest::parse(&format!(
        "[plugin]\nid = \"{id}\"\nname = \"Guest\"\nversion = \"0.1.0\"\n\
         api = \"0.1\"\nworlds = [\"{world}\"]\n",
    ))
    .expect("a valid manifest")
}

/// The fixture project every run here starts from: one sequence with one
/// video track, built through commands like the CLI's own `new`.
fn fixture() -> Project {
    let engine = Engine::spawn(Project::new("Fixture")).expect("an engine");
    let created = engine
        .handle()
        .apply(CreateSequence::new("Main", SequenceSettings::default()))
        .expect("a sequence");
    let sequence = created
        .project
        .sequences
        .first()
        .expect("the sequence that was just created")
        .id;
    engine
        .handle()
        .apply(AddTrack::new(sequence, "V1", TrackKind::Video))
        .expect("a track");
    let project = engine.handle().snapshot().as_ref().clone();
    engine.shutdown().expect("the fixture engine stops");
    project
}

/// Runs one component through the harness over a fresh fixture project.
fn test_plugin(id: &str, world: &str, wasm: &Path) -> TestReport {
    let engine = Engine::spawn(fixture()).expect("an engine");
    let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
    let harness = Harness::new(engine.handle().clone(), dispatcher).expect("a wasm compiler");
    let plugin = PluginId::parse(id).expect("a plugin id");
    let report = harness
        .run(&plugin, &manifest(id, world), Path::new("."), wasm)
        .expect("the harness ran");
    engine.shutdown().expect("the engine stops");
    report
}

/// The one check with this name, whichever world it belongs to.
fn check<'a>(report: &'a TestReport, name: &str) -> &'a sub_plugin::harness::Check {
    report
        .checks
        .iter()
        .find(|check| check.name == name)
        .unwrap_or_else(|| panic!("no {name} check in {:?}", report.checks))
}

#[test]
fn a_command_plugin_is_tested_by_asserting_project_state_after_the_run() {
    let report = test_plugin("com.example.editor", "command", guests::editor());
    assert!(report.ok(), "{:#?}", report.checks);
    assert_eq!(report.failed(), 0);

    // The component loaded, instantiated and answered JSON.
    assert_eq!(check(&report, "component_loads").status, CheckStatus::Pass);
    assert_eq!(check(&report, "instantiates").status, CheckStatus::Pass);
    let answered = check(&report, "answers_json");
    assert_eq!(answered.status, CheckStatus::Pass);
    assert_eq!(answered.detail["answer"]["track"], "V2");

    // The plugin's edit is visible in the project state the report carries:
    // one track before, two after.
    let state = check(&report, "project_state");
    assert_eq!(state.status, CheckStatus::Pass);
    assert_eq!(state.detail["changed"], true);
    assert_eq!(state.detail["before"]["tracks"], 1);
    assert_eq!(state.detail["after"]["tracks"], 2);

    // And it went on the undo stack, because it went through the Command API.
    assert_eq!(check(&report, "undoable").status, CheckStatus::Pass);

    // Whatever the plugin logged reached the report.
    assert!(
        report
            .log
            .iter()
            .any(|line| line.message.contains("editor guest starting")),
        "{:?}",
        report.log,
    );

    let json = report.to_json().expect("the report serialises");
    assert_eq!(json["ok"], true);
    assert_eq!(json["plugin"], "com.example.editor");
    assert_eq!(json["worlds"][0], "command");
    assert!(
        json["summary"]
            .as_str()
            .expect("a summary")
            .contains("0 failed")
    );
}

#[test]
fn a_command_plugin_that_never_returns_fails_with_its_stable_code() {
    let engine = Engine::spawn(fixture()).expect("an engine");
    let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
    let harness = Harness::new(engine.handle().clone(), dispatcher)
        .expect("a wasm compiler")
        // A budget small enough that the guest's endless loop is stopped in
        // well under a second.
        .with_limits(sub_plugin::Limits::default().with_fuel(1_000_000));
    let plugin = PluginId::parse("com.example.looper").expect("a plugin id");
    let report = harness
        .run(
            &plugin,
            &manifest("com.example.looper", "command"),
            Path::new("."),
            guests::looper(),
        )
        .expect("the harness ran");
    engine.shutdown().expect("the engine stops");

    assert!(!report.ok(), "{:#?}", report.checks);
    let run = check(&report, "run");
    assert_eq!(run.status, CheckStatus::Fail);
    let error = run.error.as_ref().expect("a structured error");
    assert_eq!(error["code"], "plugin.fuel_exhausted");
    // The catalogue's hint travels with it, which is what an agent acts on.
    assert!(error["details"]["hint"].is_string(), "{error:#}");
    assert_eq!(report.to_json().expect("json")["ok"], false);
}

#[test]
fn a_component_that_is_not_one_fails_the_load_check_and_nothing_else() {
    let bad = std::env::temp_dir().join("sub-plugin-harness-not-a-component.wasm");
    std::fs::write(&bad, b"\0asm\x01\0\0\0").expect("a core module header");
    let report = test_plugin("com.example.broken", "command", &bad);
    std::fs::remove_file(&bad).ok();

    assert!(!report.ok());
    assert_eq!(report.checks.len(), 1);
    let loads = check(&report, "component_loads");
    assert_eq!(loads.status, CheckStatus::Fail);
    assert_eq!(
        loads.error.as_ref().expect("an error")["code"],
        "plugin.load_failed",
    );
}

#[test]
fn an_effect_plugin_is_tested_by_rendering_a_frame() {
    let report = test_plugin("com.example.tinter", "effect", guests::tinter());

    let described = check(&report, "describe");
    assert_eq!(described.status, CheckStatus::Pass);
    assert_eq!(described.detail["params"], 1);
    assert_eq!(described.detail["entry"], "fs_scale");
    assert_eq!(check(&report, "declaration").status, CheckStatus::Pass);

    let rendered = check(&report, "renders_frame");
    match rendered.status {
        CheckStatus::Pass => {
            // The shader scales the picture down, so the frame that came back
            // is not the one that went in.
            assert_eq!(rendered.detail["changed_the_picture"], true);
            assert!(
                rendered.detail["output"]["r"].as_u64().expect("a red code")
                    < u64::from(sub_render::PROBE_INPUT[0]),
                "{:#}",
                rendered.detail["output"],
            );
            assert!(report.ok(), "{:#?}", report.checks);
        }
        // A machine with no usable adapter says so rather than failing the
        // build on an environment problem, exactly as the compositor's own
        // golden tests do.
        CheckStatus::Skip => eprintln!("skipping the frame: {}", rendered.message),
        CheckStatus::Fail => panic!("the effect did not render: {rendered:#?}"),
    }
}

#[test]
fn a_plugin_whose_manifest_declares_a_world_it_does_not_export_fails_to_instantiate() {
    // The editor guest is a `command` plugin; asked for the `effect` world's
    // export, it does not instantiate, and the report says which check failed
    // rather than ending the run.
    let report = test_plugin("com.example.editor", "effect", guests::editor());
    assert!(!report.ok(), "{:#?}", report.checks);
    let instantiates = check(&report, "instantiates");
    assert_eq!(instantiates.status, CheckStatus::Fail);
    assert_eq!(
        instantiates.error.as_ref().expect("an error")["code"],
        "plugin.instantiate_failed",
    );
}

/// The fixture above with one 15-frame clip on its video track, so an
/// expectation has something to be exact about.
fn fixture_with_a_clip() -> Project {
    let mut project = fixture();
    let sequence = project
        .sequences
        .first_mut()
        .expect("the fixture's sequence");
    let rate = sequence.settings.frame_rate;
    let range = sub_time::TimeRange::new(
        sub_time::RationalTime::from_frames(0, rate),
        sub_time::RationalTime::from_frames(15, rate),
    )
    .expect("a source range");
    sequence
        .tracks
        .first_mut()
        .expect("V1")
        .items
        .push(sub_model::TrackItem::Clip(sub_model::Clip::new(
            "shot-a",
            sub_model::MediaId::new(),
            range,
        )));
    project
}

/// An expectation over the clip laid out above, stated in `rate` units: two
/// tracks, the first holding one clip of `frames` frames at `start`.
fn expectation(start: i64, frames: i64, rate: sub_time::Rational) -> TimelineExpectation {
    TimelineExpectation {
        sequences: vec![SequenceExpectation {
            name: Some("Main".to_owned()),
            tracks: vec![
                TrackExpectation {
                    name: Some("V1".to_owned()),
                    kind: Some(TrackKind::Video),
                    clips: vec![ClipExpectation {
                        name: Some("shot-a".to_owned()),
                        start: sub_time::RationalTime::from_frames(start, rate),
                        duration: sub_time::RationalTime::from_frames(frames, rate),
                    }],
                },
                TrackExpectation {
                    name: Some("V2".to_owned()),
                    kind: Some(TrackKind::Video),
                    clips: Vec::new(),
                },
            ],
        }],
    }
}

/// Runs the editor guest over a fixture that declares `expected`.
fn test_against(expected: Option<TimelineExpectation>) -> TestReport {
    let engine = Engine::spawn(fixture_with_a_clip()).expect("an engine");
    let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
    let mut harness = Harness::new(engine.handle().clone(), dispatcher).expect("a wasm compiler");
    if let Some(expected) = expected {
        harness = harness.with_expectation(expected);
    }
    let id = "com.example.editor";
    let plugin = PluginId::parse(id).expect("a plugin id");
    let report = harness
        .run(
            &plugin,
            &manifest(id, "command"),
            Path::new("."),
            guests::editor(),
        )
        .expect("the harness ran");
    engine.shutdown().expect("the engine stops");
    report
}

/// A fixture that states the timeline it expects gets it checked, at whatever
/// timebase it stated it at: 15 frames at 24 fps is 30 at 48 (TASK-129).
#[test]
fn a_fixture_that_declares_the_timeline_it_expects_has_it_asserted() {
    let rate = sub_model::SequenceSettings::default().frame_rate;
    let report = test_against(Some(expectation(0, 15, rate)));
    assert!(report.ok(), "{:#?}", report.checks);
    let matched = check(&report, "timeline_matches");
    assert_eq!(matched.status, CheckStatus::Pass, "{matched:?}");

    let doubled = sub_time::Rational::new(rate.numerator() * 2, rate.denominator())
        .expect("twice the fixture's rate");
    let report = test_against(Some(expectation(0, 30, doubled)));
    assert_eq!(
        check(&report, "timeline_matches").status,
        CheckStatus::Pass,
        "the same instants at another timebase are the same timeline",
    );
}

/// The failure the counts could not see: the clip is not the length the
/// fixture said it would be, so the run fails and names both times exactly.
#[test]
fn a_timeline_that_is_not_what_the_fixture_declared_fails_the_run() {
    let rate = sub_model::SequenceSettings::default().frame_rate;
    let report = test_against(Some(expectation(0, 25, rate)));
    assert!(!report.ok(), "{:#?}", report.checks);
    let matched = check(&report, "timeline_matches");
    assert_eq!(matched.status, CheckStatus::Fail);
    let mismatches = matched.detail["mismatches"]
        .as_array()
        .expect("the mismatches");
    assert_eq!(mismatches.len(), 1, "{mismatches:#?}");
    let only = mismatches[0].as_str().expect("a mismatch");
    assert!(only.contains("clips[0].duration"), "{only}");
    assert!(only.contains("25@"), "{only}");
    assert!(only.contains("15@"), "{only}");
    // The rest of the command world's checks still ran and still passed.
    assert_eq!(check(&report, "project_state").status, CheckStatus::Pass);
    assert_eq!(check(&report, "undoable").status, CheckStatus::Pass);
}

/// And a fixture that declares nothing runs exactly as it did before: the
/// timeline check is skipped, and the report is still ok.
#[test]
fn a_fixture_that_declares_no_expectation_is_unchanged() {
    let report = test_against(None);
    assert!(report.ok(), "{:#?}", report.checks);
    assert_eq!(report.failed(), 0);
    assert_eq!(check(&report, "timeline_matches").status, CheckStatus::Skip,);
    assert_eq!(check(&report, "project_state").status, CheckStatus::Pass);
    assert_eq!(check(&report, "undoable").status, CheckStatus::Pass);
}
