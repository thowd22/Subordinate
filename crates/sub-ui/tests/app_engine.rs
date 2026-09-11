//! The assembled editor, driven the way a user drives it.
//!
//! Every other suite here paints one panel over a project the test owns. This
//! one builds the real [`SubordinateApp`] through `egui_kittest`'s eframe
//! harness — the same window body, the same dock, the same keyboard map — and
//! asserts on the project through the Command API rather than on the app's own
//! fields, so what it proves is that the UI and the agent surface are looking
//! at one project on one engine (docs/PLAN.md §4).
//!
//! Three things are checked:
//!
//! - A cut asked for on the timeline reaches the engine and is undone from the
//!   Edit menu, both visible through `project.get`.
//! - An edit applied through the Command API — which is the path the MCP
//!   bridge takes — shows up in the running window on the next frame.
//! - The window opens the sample project through that engine, which is what
//!   the Xvfb window smoke photographs.
//!
//! The project is copied into a temporary folder first: opening one starts the
//! autosave worker, and its snapshots live in a sidecar directory beside the
//! file.

mod support;

use std::path::{Path, PathBuf};

use eframe::egui::{self, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use sub_command::dispatch::PROJECT_GET;
use sub_edit::commands::RenameSequence;
use sub_model::{Project, TrackItem, TrackKind};
use sub_time::RationalTime;
use sub_ui::selection::ClipRef;
use sub_ui::{AppOptions, SubordinateApp};

/// A folder of this test's own, emptied first so a rerun starts clean.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-ui-app-engine-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary folder");
    dir
}

/// The committed sample project, copied somewhere the autosave sidecar may be
/// written.
fn project_copy(name: &str) -> PathBuf {
    let dir = temp_dir(name);
    let path = dir.join("sample-project.sub");
    std::fs::copy(support::fixture_path(), &path).expect("the fixture copies");
    path
}

/// The window this suite drives, in logical points.
///
/// Bigger than the panel suites' 800x600 frame on purpose: this paints the
/// whole editor rather than one panel, and the menu bar carries the adapter
/// line, so a narrow frame clips the File and Edit menus out of the window
/// (and out of the accessibility tree the test clicks through).
const WINDOW_SIZE: egui::Vec2 = egui::vec2(1400.0, 900.0);

/// The assembled editor, opened on `project`.
fn app_harness(project: &Path) -> Harness<'static, SubordinateApp> {
    let options = AppOptions {
        project: Some(project.to_path_buf()),
        ..AppOptions::default()
    };
    support::builder::<SubordinateApp>()
        .with_size(WINDOW_SIZE)
        .build_eframe(move |cc| SubordinateApp::new(cc, options).expect("the editor starts"))
}

/// The project as the Command API reports it: the revision, and the project
/// parsed back out of the project-file document `project.get` returns.
fn project_through_api(app: &SubordinateApp) -> (u64, Project) {
    // The window's own Command API client, not one the test built: what is
    // asserted here is the surface an agent talks to.
    let value = app
        .session()
        .commands()
        .invoke(PROJECT_GET, None)
        .expect("project.get answers");
    let revision = value["revision"].as_u64().expect("a revision");
    let document = serde_json::to_string(&value["project"]).expect("the document re-serialises");
    let project = sub_model::json::from_json(&document).expect("project.get returns a project");
    (revision, project)
}

/// The clips of the first video track of the first sequence, as spans.
fn video_spans(project: &Project) -> Vec<(i64, i64)> {
    let sequence = project.sequences.first().expect("a sequence");
    let rate = sequence.settings.frame_rate;
    let track = sequence
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Video)
        .expect("a video track");
    let mut spans = Vec::new();
    let mut at = RationalTime::zero(rate);
    for item in &track.items {
        let duration = item.track_duration(rate).rescaled_to(rate);
        if matches!(item, TrackItem::Clip(_)) {
            spans.push((at.rescaled_to(rate).value(), duration.value()));
        }
        at = at + duration;
    }
    spans
}

/// A frame inside the first clip of the first video track, at that sequence's
/// timebase.
fn inside_first_clip(project: &Project) -> RationalTime {
    let sequence = project.sequences.first().expect("a sequence");
    let rate = sequence.settings.frame_rate;
    let (start, duration) = *video_spans(project).first().expect("a clip to cut");
    assert!(
        duration >= 2,
        "the fixture's first clip is longer than a frame"
    );
    RationalTime::new(start + duration / 2, rate)
}

#[test]
fn a_cut_made_on_the_timeline_is_undone_from_the_edit_menu() {
    if !support::can_render() {
        return;
    }
    let path = project_copy("cut");
    let mut harness = app_harness(&path);
    harness.run();

    let (revision_before, project_before) = project_through_api(harness.state());
    assert_eq!(
        revision_before, 0,
        "opening a file is not an edit, so nothing is on the undo stack yet"
    );
    let spans_before = video_spans(&project_before);

    // Ctrl+K cuts at the playhead. Nothing is selected, so the cut goes
    // through every clip under it; the keyboard map hands the request to the
    // timeline panel, which plans it, and the window applies the plan.
    let cut_at = inside_first_clip(&project_before);
    harness.state_mut().viewer().state.seek_to(cut_at);
    harness.run();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::K);
    harness.run();
    harness.run();

    let (revision_after, project_after) = project_through_api(harness.state());
    let spans_after = video_spans(&project_after);
    assert!(
        spans_after.len() > spans_before.len(),
        "the cut split a clip: {spans_before:?} -> {spans_after:?}"
    );
    assert!(
        spans_after
            .iter()
            .any(|(start, _)| *start == cut_at.value()),
        "a clip starts at the cut: {spans_after:?}"
    );
    assert!(revision_after > revision_before, "the engine applied it");

    // The Edit menu names the step it would reverse, which is how the test
    // finds it without knowing where the menu was drawn.
    harness.get_by_label("Edit").click();
    harness.run();
    harness.get_by_label_contains("Undo ").click();
    harness.run();
    harness.run();

    let (_, project_undone) = project_through_api(harness.state());
    assert_eq!(
        video_spans(&project_undone),
        spans_before,
        "one undo rejoined every clip the cut went through"
    );
}

#[test]
fn an_edit_made_through_the_command_api_reaches_the_window() {
    if !support::can_render() {
        return;
    }
    let path = project_copy("mcp");
    let mut harness = app_harness(&path);
    harness.run();

    let sequence = harness
        .state()
        .project()
        .sequences
        .first()
        .expect("a sequence")
        .id;
    // This is the path the MCP bridge and the CLI take: a command on the same
    // engine handle the window holds, from another thread's point of view.
    harness
        .state()
        .session()
        .handle()
        .apply(RenameSequence::new(sequence, "Renamed by an agent"))
        .expect("the rename applies");

    // The window has not been told anything; it reads the change off its
    // event subscription on the next frame.
    harness.run();
    assert_eq!(
        harness.state().sequence().name,
        "Renamed by an agent",
        "the sequence the panels show followed the engine"
    );
    assert_eq!(
        harness
            .state()
            .project()
            .sequences
            .first()
            .expect("a sequence")
            .name,
        "Renamed by an agent"
    );
}

#[test]
fn the_window_opens_the_sample_project_through_the_engine() {
    if !support::can_render() {
        return;
    }
    let path = project_copy("open");
    let mut harness = app_harness(&path);
    harness.run();

    assert_eq!(
        harness.state().project_state(),
        sub_ui::ProjectState::Loaded,
        "the ready line the window smoke greps for reports a loaded project"
    );
    assert_eq!(harness.state().project_file(), Some(path.as_path()));
    let (_, through_api) = project_through_api(harness.state());
    assert!(
        !through_api.sequences.is_empty(),
        "the engine owns the project the window opened"
    );
    // These two are what `scripts/ui-smoke.sh` reads off the ready line to
    // tell a window showing the sample project from one showing an empty one.
    assert!(
        !harness.state().sequence().tracks.is_empty(),
        "the sequence the panels show has the fixture's tracks"
    );
    assert_eq!(
        through_api.sequences.first().map(|s| s.id),
        harness.state().project().sequences.first().map(|s| s.id),
        "the window and the Command API are looking at one project"
    );
}

/// The opacity of `clip`, in micro-units, wherever it sits in `sequence`.
fn opacity_micros(sequence: &sub_model::Sequence, clip: sub_model::ClipId) -> i64 {
    sequence
        .tracks
        .iter()
        .flat_map(sub_model::Track::clips)
        .find(|candidate| candidate.id == clip)
        .expect("the clip is still in the sequence")
        .opacity
        .factor()
        .micros()
}

/// The first clip on an unlocked video track of the sequence the window shows.
fn first_video_clip(app: &SubordinateApp) -> (sub_model::TrackId, sub_model::ClipId) {
    let track = app
        .sequence()
        .tracks
        .iter()
        .find(|track| {
            track.kind == TrackKind::Video && !track.locked && track.clips().next().is_some()
        })
        .expect("the sample project has an editable video track with a clip");
    (track.id, track.clips().next().expect("a clip").id)
}

#[test]
fn an_inspector_drag_edits_the_viewer_live_and_lands_as_one_undo_step() {
    if !support::can_render() {
        return;
    }
    let path = project_copy("inspector");
    let mut harness = app_harness(&path);
    harness.run();

    // The inspector edits whatever the timeline has selected, so the clip is
    // selected the way a click on the timeline selects it.
    let (track, clip) = first_video_clip(harness.state());
    harness
        .state_mut()
        .timeline()
        .selection_mut()
        .toggle(ClipRef::new(track, clip));
    harness.run();
    let before = opacity_micros(harness.state().sequence(), clip);

    // The Opacity slider's track. A slider publishes two nodes under the same
    // label — the track and the number beside it — and the track is first.
    let rect = harness
        .get_all_by_label("Opacity")
        .next()
        .expect("the docked inspector paints the opacity field")
        .rect();
    let handle = egui::pos2(rect.right() - 4.0, rect.center().y);
    harness.hover_at(handle);
    harness.run();
    harness.drag_at(handle);
    harness.run();

    // A quarter of the way along the track, with the button still down.
    let target = egui::pos2(rect.left() + rect.width() * 0.25, rect.center().y);
    harness.hover_at(target);
    harness.run();

    // Live: the sequence the compositor draws from carries the new value
    // already, and so does the project on the engine, because the window
    // applied the command on the frame the slider moved.
    let live = opacity_micros(harness.state().sequence(), clip);
    assert!(
        live < before,
        "the drag changed the clip the viewer composites while the pointer is still down: \
         {before} -> {live}"
    );
    let (_, engine_project) = project_through_api(harness.state());
    assert_eq!(
        opacity_micros(engine_project.sequences.first().expect("a sequence"), clip),
        live,
        "the engine every other client reads is holding the same live value"
    );
    let mid_drag = harness
        .state()
        .session()
        .history()
        .expect("the engine reports a history");
    assert_eq!(
        mid_drag.undo_len, 0,
        "and nothing has landed on the undo stack yet"
    );
    assert!(mid_drag.in_group, "the gesture's history group is open");

    harness.drop_at(target);
    harness.run();
    harness.run();

    let committed = harness
        .state()
        .session()
        .history()
        .expect("the engine reports a history");
    assert_eq!(
        committed.undo_len, 1,
        "releasing the slider committed the whole drag as one undoable command"
    );
    assert_eq!(committed.undo_label.as_deref(), Some("Change opacity"));
    assert_eq!(
        opacity_micros(harness.state().sequence(), clip),
        live,
        "and kept the value the drag reached"
    );

    // One undo from the Edit menu puts the whole gesture back.
    harness.get_by_label("Edit").click();
    harness.run();
    harness.get_by_label_contains("Undo ").click();
    harness.run();
    harness.run();
    assert_eq!(
        opacity_micros(harness.state().sequence(), clip),
        before,
        "one undo reverses the whole drag"
    );
    let (_, undone) = project_through_api(harness.state());
    assert_eq!(
        opacity_micros(undone.sequences.first().expect("a sequence"), clip),
        before,
        "on the engine as well as in the window"
    );
}
