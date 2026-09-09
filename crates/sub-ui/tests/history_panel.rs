//! The Edit menu and the history panel, driven headlessly over a real
//! history.
//!
//! `egui::Context::run_ui` lays a frame out on the CPU with no window and no
//! GPU, so the menu and the panel can be clicked on a CI runner. Every jump
//! the panel asks for is performed against a real [`History`] holding real
//! track commands, because what matters is that the project ends up where the
//! clicked row says it should.

use eframe::egui;
use sub_edit::History;
use sub_edit::commands::{AddTrack, RenameTrack, SetTrackMuted};
use sub_model::sequence::SequenceSettings;
use sub_model::{Project, Sequence, SequenceId, TrackKind};
use sub_ui::history_panel::{
    HistoryAction, HistoryList, HistoryPanel, ORIGINAL_STATE_LABEL, edit_menu_ui,
};
use sub_ui::shortcuts::ShortcutMap;

/// A project with one empty sequence.
fn project() -> (Project, SequenceId) {
    let mut project = Project::new("history");
    let sequence = Sequence::new("Main", SequenceSettings::default());
    let id = sequence.id;
    project.sequences.push(sequence);
    (project, id)
}

/// Applies the three commands the tests jump around in, and returns their
/// labels in the order they were applied.
fn three_edits(project: &mut Project, history: &mut History, sequence: SequenceId) -> Vec<String> {
    history
        .apply(project, AddTrack::new(sequence, "V1", TrackKind::Video))
        .expect("add track");
    let track = project.sequences[0].tracks[0].id;
    history
        .apply(project, RenameTrack::new(sequence, track, "Picture"))
        .expect("rename track");
    history
        .apply(project, SetTrackMuted::new(sequence, track, true))
        .expect("mute track");
    HistoryList::from_history(history).labels().to_vec()
}

/// Runs one headless frame drawing `body`.
fn frame<R>(ctx: &egui::Context, mut body: impl FnMut(&mut egui::Ui) -> R) -> R {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(600.0, 400.0),
        )),
        ..Default::default()
    };
    let mut result = None;
    let mut output = ctx.run_ui(input, |ui| {
        result = Some(body(ui));
    });
    let _ = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
    output.textures_delta.clear();
    result.expect("the frame body ran")
}

#[test]
fn the_list_names_every_command_and_marks_where_the_project_is() {
    let (mut project, sequence) = project();
    let mut history = History::new();
    let labels = three_edits(&mut project, &mut history, sequence);
    assert_eq!(labels.len(), 3, "labels: {labels:?}");

    let list = HistoryList::from_history(&history);
    assert_eq!(list.position(), 3);
    assert_eq!(list.undo_label(), Some(labels[2].as_str()));
    assert_eq!(list.redo_label(), None);
    assert_eq!(list.undo_menu_label(), format!("Undo {}", labels[2]));

    // After an undo the same step is on the other side of the position, and
    // the list still lists it.
    history.undo(&mut project).expect("undo");
    let list = HistoryList::from_history(&history);
    assert_eq!(list.labels(), labels.as_slice());
    assert_eq!(list.position(), 2);
    assert_eq!(list.redo_label(), Some(labels[2].as_str()));
    assert_eq!(list.redo_menu_label(), format!("Redo {}", labels[2]));
    assert!(list.is_applied(1));
    assert!(!list.is_applied(2));
}

#[test]
fn the_edit_menu_paints_and_invents_nothing_on_its_own() {
    let (mut project, sequence) = project();
    let mut history = History::new();
    three_edits(&mut project, &mut history, sequence);
    let list = HistoryList::from_history(&history);
    let shortcuts = ShortcutMap::default_map();
    let ctx = egui::Context::default();

    // Two frames: egui needs a second pass before its widgets have their real
    // sizes, and neither pass may ask for an undo nobody clicked.
    for _ in 0..2 {
        let action = frame(&ctx, |ui| edit_menu_ui(ui, &list, &shortcuts));
        assert_eq!(action, None);
    }
}

#[test]
fn the_panel_paints_a_row_per_step_and_asks_for_nothing_unclicked() {
    let (mut project, sequence) = project();
    let mut history = History::new();
    three_edits(&mut project, &mut history, sequence);
    let list = HistoryList::from_history(&history);
    let ctx = egui::Context::default();
    let mut panel = HistoryPanel::new();

    for _ in 0..2 {
        let action = frame(&ctx, |ui| panel.ui(ui, &list));
        assert_eq!(action, None);
    }
    // One row per step, plus the open-state row above them.
    assert!(panel.row_rect(list.len()).is_some());
    assert_eq!(panel.row_rect(list.len() + 1), None);

    assert!(!panel.open);
    assert_eq!(
        panel.show(&ctx, &list),
        None,
        "a closed panel draws nothing"
    );
    panel.toggle();
    assert!(panel.open);
    for _ in 0..2 {
        let action = frame(&ctx, |ui| panel.show(ui.ctx(), &list));
        assert_eq!(action, None);
    }
    assert!(panel.open, "the window stays open until it is closed");
}

#[test]
fn jumping_back_and_forward_lands_on_the_clicked_step() {
    let (mut project, sequence) = project();
    let mut history = History::new();
    three_edits(&mut project, &mut history, sequence);
    let track = project.sequences[0].tracks[0].id;

    // Jump to the point just after the first command: one track, its original
    // name, unmuted.
    let list = HistoryList::from_history(&history);
    let back = HistoryAction::to_position(&list, 1).expect("a jump back");
    assert_eq!(back, HistoryAction::Undo { steps: 2 });
    assert_eq!(back.perform(&mut history, &mut project).expect("undo"), 2);
    let state = &project.sequences[0];
    assert_eq!(state.tracks.len(), 1);
    assert_eq!(state.tracks[0].name, "V1");
    assert!(!state.tracks[0].muted);
    assert_eq!(state.tracks[0].id, track);

    // And forward again, to the end.
    let list = HistoryList::from_history(&history);
    assert_eq!(list.position(), 1);
    let forward = HistoryAction::to_position(&list, list.len()).expect("a jump forward");
    assert_eq!(forward, HistoryAction::Redo { steps: 2 });
    assert_eq!(
        forward.perform(&mut history, &mut project).expect("redo"),
        2
    );
    let state = &project.sequences[0];
    assert_eq!(state.tracks[0].name, "Picture");
    assert!(state.tracks[0].muted);

    // The first row undoes everything.
    let list = HistoryList::from_history(&history);
    assert_eq!(list.labels().len(), 3);
    let to_open = HistoryAction::to_position(&list, 0).expect("a jump to the open state");
    assert_eq!(
        to_open.perform(&mut history, &mut project).expect("undo"),
        3
    );
    assert!(project.sequences[0].tracks.is_empty());
    assert!(!HistoryList::from_history(&history).can_undo());
}

#[test]
fn clicking_a_row_in_a_painted_panel_asks_for_that_jump() {
    let (mut project, sequence) = project();
    let mut history = History::new();
    three_edits(&mut project, &mut history, sequence);
    let list = HistoryList::from_history(&history);
    let ctx = egui::Context::default();
    let mut panel = HistoryPanel::new();

    // Lay the panel out twice so the rows have their real rects, then click
    // the row for the first command.
    for _ in 0..2 {
        frame(&ctx, |ui| panel.ui(ui, &list));
    }
    let target = panel.row_rect(1).expect("the first command has a row");
    let click = target.center();

    let mut action = None;
    for events in press_and_release(click) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 400.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            if let Some(next) = panel.ui(ui, &list) {
                action = Some(next);
            }
        });
        let _ = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        output.textures_delta.clear();
    }
    assert_eq!(action, Some(HistoryAction::Undo { steps: 2 }));

    // Performing it leaves the project where the clicked row says.
    action
        .expect("a jump")
        .perform(&mut history, &mut project)
        .expect("undo");
    assert_eq!(project.sequences[0].tracks[0].name, "V1");
    assert!(!project.sequences[0].tracks[0].muted);
}

/// The events of a click at `pos`, one frame's worth per element.
fn press_and_release(pos: egui::Pos2) -> Vec<Vec<egui::Event>> {
    let press = egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    };
    let release = egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    };
    vec![
        vec![egui::Event::PointerMoved(pos)],
        vec![press],
        vec![release],
    ]
}

#[test]
fn a_new_command_clears_the_redo_side_of_the_list() {
    let (mut project, sequence) = project();
    let mut history = History::new();
    let labels = three_edits(&mut project, &mut history, sequence);
    let track = project.sequences[0].tracks[0].id;

    history.undo(&mut project).expect("undo");
    history.undo(&mut project).expect("undo");
    let list = HistoryList::from_history(&history);
    assert_eq!(list.position(), 1);
    assert_eq!(list.len(), 3);
    assert!(list.can_redo());

    // A command applied at that position replaces everything after it, so the
    // panel drops the two redoable rows and lists the new one instead.
    history
        .apply(&mut project, RenameTrack::new(sequence, track, "Cutaway"))
        .expect("rename track");
    let list = HistoryList::from_history(&history);
    assert!(!list.can_redo(), "the redo stack should be gone");
    assert_eq!(list.position(), 2);
    assert_eq!(list.len(), 2);
    assert_eq!(list.labels()[0], labels[0]);
    assert_ne!(list.labels()[1], labels[2]);
    assert_eq!(list.redo_menu_label(), "Redo");
    assert_eq!(HistoryAction::to_position(&list, 3), None);
}

#[test]
fn a_jump_past_the_end_of_a_stale_list_stops_at_the_end() {
    let (mut project, sequence) = project();
    let mut history = History::new();
    three_edits(&mut project, &mut history, sequence);

    // A list drawn before someone else emptied the history: performing its
    // jump undoes what is there and stops, rather than failing.
    let stale = HistoryList::from_history(&history);
    history.clear();
    let action = HistoryAction::to_position(&stale, 0).expect("a jump back");
    assert_eq!(action, HistoryAction::Undo { steps: 3 });
    assert_eq!(action.perform(&mut history, &mut project).expect("undo"), 0);
}

#[test]
fn the_open_state_row_is_named() {
    assert_eq!(ORIGINAL_STATE_LABEL, "Open state");
}
