//! Driving the relink dialog headlessly against real files.
//!
//! `egui::Context::run_ui` runs a whole frame with no window and no GPU, so
//! the dialog can be opened, told to search a folder of real footage stand-ins,
//! and clicked, and what it hands back can be applied through the history and
//! undone. The contract under test is the one the user sees: a moved file is
//! found by its bytes, every offline item is relinked at once, and one press
//! of undo puts the project back.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Pos2, Rect};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sub_core::{JobService, Priority};
use sub_edit::History;
use sub_edit::relink::MatchKind;
use sub_model::{ContentHash, MediaId, MediaItem, MediaPath, Project};
use sub_ui::media_bin::{MediaBinAction, MediaBinPanel};
use sub_ui::relink_dialog::RelinkDialog;

/// A directory of this test's own, emptied first so a rerun starts clean.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-ui-relink-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the temporary directory is creatable");
    dir
}

/// Writes `bytes` to `dir/relative`, creating the folders on the way.
fn write(dir: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(relative);
    std::fs::create_dir_all(path.parent().expect("a file has a parent"))
        .expect("the folder is creatable");
    std::fs::write(&path, bytes).expect("the file is writable");
    path
}

/// Adds an offline item for `relative`, hashed as `hash`, to `project`.
fn offline_item(project: &mut Project, relative: &str, hash: Option<ContentHash>) -> MediaId {
    let mut item = MediaItem::new(MediaPath::new(relative).expect("a valid relative path"));
    item.hash = hash;
    item.offline = true;
    let id = item.id;
    project.root_bin.media.push(id);
    project.media.push(item);
    id
}

/// Runs one frame of the dialog, feeding it `events`.
fn frame(
    ctx: &egui::Context,
    dialog: &mut RelinkDialog,
    jobs: &JobService,
    project: &Project,
    project_dir: &Path,
    events: Vec<egui::Event>,
) -> (
    Option<sub_edit::relink::RelinkPlan>,
    Vec<egui::epaint::ClippedShape>,
) {
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 600.0))),
        events,
        ..Default::default()
    };
    let mut plan = None;
    let mut output = ctx.run_ui(input, |ui| {
        plan = dialog.ui(ui.ctx(), jobs, project, project_dir);
    });
    let shapes = std::mem::take(&mut output.shapes);
    output.textures_delta.clear();
    (plan, shapes)
}

/// The events that click at `pos`.
fn click_at(pos: Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        },
    ]
}

/// Every piece of text in `shapes`, however deeply nested.
fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    fn walk(shape: &egui::Shape, into: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => into.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, into);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut found);
    }
    found
}

/// Where the text `wanted` was painted, if it was.
fn text_center(shapes: &[egui::epaint::ClippedShape], wanted: &str) -> Option<Pos2> {
    fn walk(shape: &egui::Shape, wanted: &str, into: &mut Vec<Pos2>) {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == wanted => {
                into.push(text.pos + text.galley.size() / 2.0);
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, wanted, into);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, wanted, &mut found);
    }
    found.first().copied()
}

#[test]
fn a_folder_search_relinks_every_offline_item_in_one_undo_step() {
    let dir = temp_dir("bulk");
    // Both takes moved into a subfolder of the project, one of them renamed.
    let one = write(
        &dir,
        "recovered/renamed.mov",
        b"take one, moved and renamed",
    );
    let two = write(&dir, "recovered/two.mov", b"take two, only moved");
    let hash_one = ContentHash::of_file(&one).expect("readable");
    let hash_two = ContentHash::of_file(&two).expect("readable");

    let mut project = Project::new("Doc cut");
    let a = offline_item(&mut project, "footage/one.mov", Some(hash_one));
    let b = offline_item(&mut project, "footage/two.mov", Some(hash_two));

    // One worker, held by a job that will not finish until the test says so,
    // so the frame drawn while the search is queued is deterministic.
    let jobs = JobService::new(1);
    let gate = Arc::new(AtomicBool::new(false));
    let held = Arc::clone(&gate);
    // Submitted at the search's own priority so it is taken first, and it
    // gives up when cancelled, so a failing assertion here cannot wedge the
    // service on the way out.
    let _blocker = jobs.submit("test.block", Priority::Interactive, move |ctx| {
        while !held.load(Ordering::SeqCst) {
            ctx.check()?;
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    });

    let ctx = egui::Context::default();
    let mut dialog = RelinkDialog::new();
    assert!(!dialog.is_open(), "a fresh dialog is closed");

    dialog.open_for_offline(&project);
    assert!(dialog.is_open());
    assert_eq!(dialog.targets().len(), 2);

    // The search runs on the job service, so the frame that starts it draws
    // "Searching..." and asks for nothing.
    dialog.search_folder(&jobs, &dir);
    assert!(dialog.is_searching());
    // A window is laid out on its first frame and painted on the next, so
    // every assertion about what is on screen reads the second frame.
    let (_, _) = frame(&ctx, &mut dialog, &jobs, &project, &dir, Vec::new());
    let (plan, shapes) = frame(&ctx, &mut dialog, &jobs, &project, &dir, Vec::new());
    assert!(plan.is_none(), "a running search proposes nothing");
    assert!(texts(&shapes).iter().any(|text| text == "Searching..."));

    gate.store(true, Ordering::SeqCst);
    jobs.wait_idle();
    assert!(dialog.poll(), "the search landed");
    assert!(!dialog.is_searching());
    assert_eq!(dialog.matches().len(), 2);
    for found in dialog.matches() {
        assert_eq!(found.kind, MatchKind::Hash, "the bytes found both files");
    }

    // The button says how many items one press would cover.
    let (plan, shapes) = frame(&ctx, &mut dialog, &jobs, &project, &dir, Vec::new());
    assert!(plan.is_none(), "nothing happens until it is clicked");
    let button = text_center(&shapes, "Relink 2 items").expect("the button was painted");
    let (plan, _) = frame(&ctx, &mut dialog, &jobs, &project, &dir, click_at(button));
    let plan = plan.expect("the click hands the plan back");
    assert_eq!(plan.len(), 2);
    assert!(!dialog.is_open(), "accepting closes the dialog");

    let mut history = History::new();
    assert_eq!(plan.apply(&mut history, &mut project).expect("applies"), 2);
    assert_eq!(project.offline_media().count(), 0);
    assert_eq!(
        project.media_item(a).expect("kept").path.as_str(),
        "recovered/renamed.mov"
    );
    assert_eq!(
        project.media_item(b).expect("kept").path.as_str(),
        "recovered/two.mov"
    );

    history.undo(&mut project).expect("one press undoes both");
    assert_eq!(project.offline_media().count(), 2);
    assert!(history.undo_label().is_none(), "it was a single step");
}

#[test]
fn picking_a_file_relinks_the_one_item_whatever_it_is_called() {
    let dir = temp_dir("single");
    let chosen = write(&dir, "graded/take-01-graded.mov", b"a graded version");

    let mut project = Project::new("Doc cut");
    let media = offline_item(&mut project, "footage/take-01.mov", None);

    let mut dialog = RelinkDialog::new();
    dialog.open_for(&project, media);
    assert_eq!(dialog.targets().len(), 1);

    dialog.choose_file(&chosen);
    assert_eq!(dialog.matches().len(), 1);
    assert_eq!(dialog.matches()[0].media, media);
    assert_eq!(
        dialog.matches()[0].kind,
        MatchKind::Name,
        "the bytes are not the ones the item remembered"
    );

    let plan = dialog.plan(&dir);
    assert_eq!(plan.len(), 1);
    let mut history = History::new();
    plan.apply(&mut history, &mut project).expect("applies");
    let item = project.media_item(media).expect("kept its identity");
    assert_eq!(item.path.as_str(), "graded/take-01-graded.mov");
    assert!(!item.offline);
    assert!(item.hash.is_some(), "the chosen file is hashed");
}

#[test]
fn a_file_outside_the_project_folder_is_named_and_refused() {
    let dir = temp_dir("outside");
    let elsewhere = temp_dir("outside-source");
    let chosen = write(&elsewhere, "take.mov", b"left outside the project folder");

    let mut project = Project::new("Doc cut");
    let media = offline_item(&mut project, "footage/take.mov", None);

    let jobs = JobService::new(1);
    let ctx = egui::Context::default();
    let mut dialog = RelinkDialog::new();
    dialog.open_for(&project, media);
    dialog.choose_file(&chosen);

    let (_, _) = frame(&ctx, &mut dialog, &jobs, &project, &dir, Vec::new());
    let (plan, shapes) = frame(&ctx, &mut dialog, &jobs, &project, &dir, Vec::new());
    assert!(plan.is_none(), "the refused file offers nothing to apply");
    let painted = texts(&shapes);
    assert!(
        painted
            .iter()
            .any(|text| text.contains("not inside the project folder")),
        "the dialog says why; it drew {painted:?}"
    );
    assert!(dialog.plan(&dir).is_empty());
    assert_eq!(dialog.plan(&dir).rejected.len(), 1);
}

#[test]
fn the_bin_offers_a_bulk_relink_while_anything_is_offline() {
    let mut project = Project::new("Doc cut");
    offline_item(&mut project, "footage/one.mov", None);

    let ctx = egui::Context::default();
    let mut panel = MediaBinPanel::new();
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 700.0))),
        ..Default::default()
    };
    let mut raised = Vec::new();
    let mut output = ctx.run_ui(input.clone(), |ui| raised = panel.ui(ui, &project));
    let shapes = std::mem::take(&mut output.shapes);
    output.textures_delta.clear();
    assert!(raised.is_empty());
    let painted = texts(&shapes);
    assert!(painted.iter().any(|text| text == "1 offline"));
    let button = text_center(&shapes, "Relink all").expect("the button was painted");

    let clicked = egui::RawInput {
        events: click_at(button),
        ..input
    };
    let mut raised = Vec::new();
    let mut output = ctx.run_ui(clicked, |ui| raised = panel.ui(ui, &project));
    output.textures_delta.clear();
    assert_eq!(raised, vec![MediaBinAction::RelinkAll]);

    // Nothing offline, nothing offered.
    project.media[0].offline = false;
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 700.0))),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        let _ = panel.ui(ui, &project);
    });
    let shapes = std::mem::take(&mut output.shapes);
    output.textures_delta.clear();
    assert!(text_center(&shapes, "Relink all").is_none());
}
