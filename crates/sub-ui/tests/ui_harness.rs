//! What the UI test harness itself guarantees.
//!
//! The panel suites (the timeline, the media bin, the viewer) live in their
//! own files and share [`support`]. This one covers the harness: that the
//! committed sample project loads, that a panel painted through it renders and
//! matches its committed snapshot, that a click routed through `AccessKit`
//! reaches a real panel, and that the committed snapshot directory stays
//! inside its size budget.

mod support;

use std::path::{Path, PathBuf};

use egui_kittest::kittest::Queryable;
use sub_ui::history_panel::{HistoryAction, HistoryList, HistoryPanel};
use sub_ui::timeline_panel::TimelinePanel;

/// The largest a single committed snapshot may be.
const MAX_SNAPSHOT_BYTES: u64 = 150 * 1024;

/// The largest the whole committed snapshot directory may be.
const MAX_SNAPSHOT_DIR_BYTES: u64 = 5 * 1024 * 1024;

/// Where committed snapshots live. Must agree with `output_path` in
/// `kittest.toml`.
fn snapshot_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

#[test]
fn the_fixture_project_loads() {
    let project = support::fixture_project();
    assert!(
        project.sequences.len() >= 2,
        "the sample project has two sequences"
    );
    let sequence = support::fixture_sequence(&project);
    assert!(!sequence.tracks.is_empty(), "its first sequence has tracks");
    assert!(!project.media.is_empty(), "and media to hang clips off");
}

#[test]
fn a_panel_renders_through_the_harness() {
    if !support::can_render() {
        return;
    }
    let mut harness = support::panel_harness(|ui| {
        ui.heading("Subordinate");
    });
    harness.run();
    let image = harness.render().expect("the harness renders");
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the panel size is a small positive whole number of points"
    )]
    let expected = (support::PANEL_SIZE.x as u32, support::PANEL_SIZE.y as u32);
    assert_eq!(
        (image.width(), image.height()),
        expected,
        "one physical pixel per point at the shared panel size"
    );
}

#[test]
fn the_timeline_panel_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let project = support::fixture_project();
    let sequence = support::fixture_sequence(&project).clone();
    let rate = sequence.settings.frame_rate;
    let mut panel = TimelinePanel::new(rate);
    let mut harness = support::panel_harness(|ui| {
        panel.sync(&sequence, 1);
        panel.ui(ui, &project, &sequence);
    });
    harness.run();
    support::snapshot(&mut harness, "harness_timeline_panel");
}

#[test]
fn a_click_reaches_the_panel_under_test() {
    // No adapter needed: `AccessKit` describes the tree egui laid out, so the
    // interaction half of the harness runs anywhere.
    let list = HistoryList::new(vec!["Add track".to_owned(), "Rename track".to_owned()], 2);
    let state = (HistoryPanel::new(), list, None::<HistoryAction>);
    let mut harness = support::panel_harness_state(state, |ui, (panel, list, action)| {
        if let Some(clicked) = panel.ui(ui, list) {
            *action = Some(clicked);
        }
    });
    harness.run();

    harness.get_by_label("Add track").click();
    harness.run();

    let (_, _, action) = harness.state();
    assert_eq!(
        *action,
        Some(HistoryAction::Undo { steps: 1 }),
        "clicking the row below the current position undoes one command"
    );
}

#[test]
fn committed_snapshots_stay_inside_their_size_budget() {
    let dir = snapshot_dir();
    if !dir.is_dir() {
        // Nothing committed yet: the budget cannot be broken.
        return;
    }
    let mut total = 0;
    for entry in std::fs::read_dir(&dir).expect("the snapshot directory reads") {
        let entry = entry.expect("a snapshot directory entry reads");
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "png") {
            continue;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if name.ends_with(".new.png") || name.ends_with(".diff.png") {
            // A failing run's output, gitignored and never committed.
            continue;
        }
        let size = entry.metadata().expect("a snapshot's metadata reads").len();
        assert!(
            size <= MAX_SNAPSHOT_BYTES,
            "{name} is {size} bytes, over the {MAX_SNAPSHOT_BYTES} byte budget for one snapshot; \
             render a smaller frame"
        );
        total += size;
    }
    assert!(
        total <= MAX_SNAPSHOT_DIR_BYTES,
        "the committed snapshots total {total} bytes, over the {MAX_SNAPSHOT_DIR_BYTES} byte \
         budget for the directory"
    );
}
