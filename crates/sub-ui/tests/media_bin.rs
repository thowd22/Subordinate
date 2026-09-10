//! Driving the media bin headlessly, and applying what it raises.
//!
//! `egui::Context::run_ui` runs a whole frame — input, layout, interaction and
//! painting — with no window and no GPU, so a click on a column heading, a
//! click on "Move here" and a file dropped from the desktop can all be
//! synthesised here, and the actions they raise can be applied through the
//! Command API and undone. That round trip is the contract of the panel: it
//! mutates nothing itself, and everything it asks for is undoable.
//!
//! The last few tests drive the same panel through the shared `egui_kittest`
//! harness instead: they click by accessibility label, and they hold the two
//! view modes to committed snapshots.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, Pos2, Rect};
use egui_kittest::kittest::Queryable;
use sub_edit::History;
use sub_edit::commands::{CreateBin, MoveBin, MoveToBin, RelinkMedia, RenameBin};
use sub_model::media::{StreamInfo, VideoStream};
use sub_model::{Bin, BinId, ColorTags, MediaId, MediaItem, MediaPath, Project};
use sub_time::{Rational, RationalTime};
use sub_ui::media_bin::{BinSelection, BinViewMode, MediaBinAction, MediaBinPanel, SortColumn};

/// A file the integration reports as dropped on the window.
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        Err("the panel never reads a dropped file".to_owned())
    }
}

/// A media item named `name`, `frames` long at `rate`.
fn item(name: &str, frames: i64, rate: Rational, width: u32, height: u32) -> MediaItem {
    let mut media = MediaItem::new(MediaPath::new(format!("footage/{name}")).expect("valid path"));
    name.clone_into(&mut media.name);
    media.info = Some(StreamInfo {
        duration: Some(RationalTime::new(frames, rate)),
        video: vec![VideoStream {
            width,
            height,
            frame_rate: rate,
            sample_aspect: Rational::ONE,
            color: ColorTags::REC709,
        }],
        audio: Vec::new(),
    });
    media
}

/// What the fixture project holds.
struct Fixture {
    /// The `Interviews` bin, a child of the root.
    interviews: BinId,
    /// `a.mov`, 25 fps, filed in the root bin.
    fast: MediaId,
    /// `b.mov`, 23.976 fps, filed in the root bin and offline.
    offline: MediaId,
}

/// A project with two items in the root bin and one empty child bin.
fn fixture() -> (Project, Fixture) {
    let mut project = Project::new("Doc cut");

    let fast = item("a.mov", 100, Rational::FPS_25, 3840, 2160);
    let fast_id = fast.id;
    project.root_bin.media.push(fast_id);
    project.media.push(fast);

    let mut missing = item("b.mov", 24, Rational::FPS_23_976, 1280, 720);
    missing.offline = true;
    let offline_id = missing.id;
    project.root_bin.media.push(offline_id);
    project.media.push(missing);

    let interviews = Bin::new("Interviews");
    let interviews_id = interviews.id;
    project.root_bin.children.push(interviews);

    (
        project,
        Fixture {
            interviews: interviews_id,
            fast: fast_id,
            offline: offline_id,
        },
    )
}

/// Runs one frame, feeding it `events` and `dropped`, and returns what the
/// panel raised plus the shapes it painted.
fn frame(
    ctx: &egui::Context,
    panel: &mut MediaBinPanel,
    project: &Project,
    events: Vec<egui::Event>,
    dropped: Vec<PathBuf>,
) -> (Vec<MediaBinAction>, Vec<egui::epaint::ClippedShape>) {
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 700.0))),
        events,
        dropped_files: dropped
            .into_iter()
            .map(|path| Arc::new(Dropped(path)) as egui::DroppedFileHandle)
            .collect(),
        ..Default::default()
    };
    let mut raised = Vec::new();
    let mut output = ctx.run_ui(input, |ui| {
        raised = panel.ui(ui, project);
    });
    let shapes = std::mem::take(&mut output.shapes);
    output.textures_delta.clear();
    (raised, shapes)
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

/// Where the text `wanted` was painted, if it was. When several were painted,
/// the `index`th one, in paint order.
fn text_center(shapes: &[egui::epaint::ClippedShape], wanted: &str, index: usize) -> Option<Pos2> {
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
    found.get(index).copied()
}

/// The names of the items, in the order the list painted them.
fn listed_names(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    texts(shapes)
        .into_iter()
        .filter(|text| text.contains(".mov"))
        .collect()
}

#[test]
fn a_file_dropped_from_the_desktop_is_an_import_into_the_open_folder() {
    let (project, at) = fixture();
    let mut panel = MediaBinPanel::new();
    let ctx = egui::Context::default();

    let (raised, _) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    assert!(raised.is_empty(), "an idle frame asks for nothing");

    let dropped = vec![PathBuf::from("/projects/cut/footage/new.mov")];
    let (raised, _) = frame(&ctx, &mut panel, &project, Vec::new(), dropped.clone());
    assert_eq!(
        raised,
        vec![MediaBinAction::Import {
            paths: dropped.clone(),
            bin: project.root_bin.id,
        }]
    );

    // The drop follows the open folder.
    panel.open(at.interviews);
    let (raised, _) = frame(&ctx, &mut panel, &project, Vec::new(), dropped.clone());
    assert_eq!(
        raised,
        vec![MediaBinAction::Import {
            paths: dropped,
            bin: at.interviews,
        }]
    );
}

#[test]
fn clicking_a_column_heading_reorders_the_list_both_ways() {
    let (project, _) = fixture();
    let mut panel = MediaBinPanel::new();
    let ctx = egui::Context::default();

    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    assert_eq!(listed_names(&shapes), ["a.mov", "b.mov"]);

    // The duration heading: b.mov is one second, a.mov four.
    let heading = text_center(&shapes, "Duration", 0).expect("the heading was painted");
    let (_, shapes) = frame(&ctx, &mut panel, &project, click_at(heading), Vec::new());
    assert_eq!(panel.sort.column, SortColumn::Duration);
    assert!(panel.sort.ascending);
    assert_eq!(listed_names(&shapes), ["b.mov", "a.mov"]);

    // The heading grows its arrow on the next frame, because the click is
    // handled after the heading it lands on has been painted.
    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    // Clicking it again reverses it, and says so in the heading.
    let heading = text_center(&shapes, "Duration ^", 0).expect("the arrow was painted");
    let (_, shapes) = frame(&ctx, &mut panel, &project, click_at(heading), Vec::new());
    assert!(!panel.sort.ascending);
    assert_eq!(listed_names(&shapes), ["a.mov", "b.mov"]);
    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    assert!(texts(&shapes).iter().any(|text| text == "Duration v"));
}

#[test]
fn the_list_shows_the_metadata_columns_and_marks_offline_items() {
    let (project, _) = fixture();
    let mut panel = MediaBinPanel::new();
    let ctx = egui::Context::default();

    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    let painted = texts(&shapes);
    for expected in [
        "Name ^",
        "Duration",
        "FPS",
        "Resolution",
        "00:00:04:00",
        "25",
        "3840x2160",
        "23.976",
        "1280x720",
        "OFFLINE",
        "Relink",
    ] {
        assert!(
            painted.iter().any(|text| text == expected),
            "{expected} was not painted; the frame drew {painted:?}"
        );
    }
}

#[test]
fn relinking_an_offline_item_is_raised_and_applied_as_a_command() {
    let (mut project, at) = fixture();
    let mut panel = MediaBinPanel::new();
    let ctx = egui::Context::default();

    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    let button = text_center(&shapes, "Relink", 0).expect("the relink button was painted");
    let (raised, _) = frame(&ctx, &mut panel, &project, click_at(button), Vec::new());
    assert_eq!(raised, vec![MediaBinAction::Relink(at.offline)]);
    assert_eq!(panel.selection(&project), BinSelection::Media(at.offline));

    // The host asks for the replacement file and applies the command; the
    // item keeps its identity, so the clips cut from it survive.
    let mut history = History::new();
    let replacement = MediaPath::new("footage/b-recovered.mov").expect("valid path");
    history
        .apply(
            &mut project,
            RelinkMedia::new(at.offline, replacement.clone()),
        )
        .expect("relink applies");
    let relinked = project
        .media_item(at.offline)
        .expect("the item is still there");
    assert_eq!(relinked.path, replacement);
    assert!(!relinked.offline);

    history.undo(&mut project).expect("relink undoes");
    assert!(project.media_item(at.offline).expect("still there").offline);

    // And the panel draws the item as offline again.
    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    assert!(texts(&shapes).iter().any(|text| text == "OFFLINE"));
}

#[test]
fn creating_and_renaming_a_folder_goes_through_the_command_api() {
    let (mut project, _) = fixture();
    let mut panel = MediaBinPanel::new();
    let ctx = egui::Context::default();
    let mut history = History::new();

    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    let button = text_center(&shapes, "New folder", 1).expect("the button was painted");
    let (raised, _) = frame(&ctx, &mut panel, &project, click_at(button), Vec::new());
    let MediaBinAction::CreateBin { parent, name } = raised
        .into_iter()
        .next()
        .expect("the panel asked for a folder")
    else {
        panic!("the panel asked for something else");
    };
    assert_eq!(parent, project.root_bin.id);
    assert_eq!(name, "New folder", "an empty name gets a default");

    history
        .apply(&mut project, CreateBin::new(name).inside(parent))
        .expect("the folder is created");
    let created = project.root_bin.children[1].id;
    panel.open(created);

    // Renaming is the same shape: type a name into the field, press the
    // button, apply what it raises.
    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    let field = text_center(&shapes, "Rename to", 0).expect("the field was painted");
    frame(&ctx, &mut panel, &project, click_at(field), Vec::new());
    frame(
        &ctx,
        &mut panel,
        &project,
        vec![egui::Event::Text("Selects".to_owned())],
        Vec::new(),
    );
    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    let button = text_center(&shapes, "Rename", 0).expect("the button was painted");
    let (raised, _) = frame(&ctx, &mut panel, &project, click_at(button), Vec::new());
    assert_eq!(
        raised,
        vec![MediaBinAction::RenameBin {
            bin: created,
            name: "Selects".to_owned(),
        }]
    );
    let MediaBinAction::RenameBin { bin, name } = raised.into_iter().next().expect("one action")
    else {
        panic!("the panel asked for something else");
    };
    history
        .apply(&mut project, RenameBin::new(bin, name))
        .expect("the folder is renamed");
    assert_eq!(project.root_bin.children[1].name, "Selects");

    history.undo(&mut project).expect("the rename undoes");
    history.undo(&mut project).expect("the creation undoes");
    assert_eq!(project.root_bin.children.len(), 1);
    assert_eq!(
        panel.open_bin(&project),
        project.root_bin.id,
        "the open folder falls back to the root once it is gone"
    );
}

#[test]
fn moving_an_item_and_a_folder_are_raised_and_undone_exactly() {
    let (mut project, at) = fixture();
    let mut panel = MediaBinPanel::new();
    let ctx = egui::Context::default();
    let mut history = History::new();

    // Selecting an item and clicking "Move here" on another folder files it
    // there.
    panel.select_media(at.fast);
    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    let move_here = text_center(&shapes, "Move here", 0).expect("the button was painted");
    let (raised, _) = frame(&ctx, &mut panel, &project, click_at(move_here), Vec::new());
    assert_eq!(
        raised,
        vec![MediaBinAction::MoveMedia {
            media: at.fast,
            bin: at.interviews,
        }]
    );

    history
        .apply(&mut project, MoveToBin::new(at.fast, at.interviews))
        .expect("the item moves");
    assert_eq!(project.bin_of(at.fast), Some(at.interviews));
    history.undo(&mut project).expect("the move undoes");
    assert_eq!(project.bin_of(at.fast), Some(project.root_bin.id));

    // A folder moves the same way. `Selects` is selected, `Interviews` is the
    // only other folder it can move into.
    history
        .apply(&mut project, CreateBin::new("Selects"))
        .expect("the folder is created");
    let selects = project.root_bin.children[1].id;
    panel.open(selects);
    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    let move_here = text_center(&shapes, "Move here", 0).expect("the button was painted");
    let (raised, _) = frame(&ctx, &mut panel, &project, click_at(move_here), Vec::new());
    assert_eq!(
        raised,
        vec![MediaBinAction::MoveBin {
            bin: selects,
            parent: at.interviews,
        }]
    );

    history
        .apply(&mut project, MoveBin::new(selects, at.interviews))
        .expect("the folder moves");
    assert_eq!(project.root_bin.children.len(), 1);
    assert_eq!(project.root_bin.children[0].children[0].id, selects);
    history.undo(&mut project).expect("the move undoes");
    assert_eq!(project.root_bin.children[1].id, selects);
}

#[test]
fn a_folder_offers_no_move_into_itself_its_subtree_or_its_own_parent() {
    let (mut project, at) = fixture();
    let mut panel = MediaBinPanel::new();
    let ctx = egui::Context::default();
    let mut history = History::new();
    history
        .apply(&mut project, CreateBin::new("Takes").inside(at.interviews))
        .expect("the folder is created");

    // With `Interviews` open, the only rows are the root (its own parent),
    // itself, and `Takes` (its child): nothing it can legally move into.
    panel.open(at.interviews);
    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    assert!(
        !texts(&shapes).iter().any(|text| text == "Move here"),
        "no illegal move was offered"
    );

    // The root bin itself never moves.
    panel.open(project.root_bin.id);
    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    assert!(!texts(&shapes).iter().any(|text| text == "Move here"));
}

#[test]
fn the_grid_view_draws_a_tile_for_every_item() {
    let (project, _) = fixture();
    let mut panel = MediaBinPanel::new();
    let ctx = egui::Context::default();

    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    let toggle = text_center(&shapes, "Grid", 0).expect("the toggle was painted");
    let (_, shapes) = frame(&ctx, &mut panel, &project, click_at(toggle), Vec::new());
    assert_eq!(panel.mode, BinViewMode::Grid);

    let painted = texts(&shapes);
    assert_eq!(listed_names(&shapes), ["a.mov", "b.mov"]);
    assert!(painted.iter().any(|text| text == "3840x2160"));
    assert!(painted.iter().any(|text| text == "OFFLINE"));
    assert!(
        !painted.iter().any(|text| text == "Resolution"),
        "the grid has no columns"
    );
}

#[test]
fn the_tree_collapses_and_the_breadcrumb_names_the_open_folder() {
    let (mut project, at) = fixture();
    let mut panel = MediaBinPanel::new();
    let ctx = egui::Context::default();
    let mut history = History::new();
    history
        .apply(&mut project, CreateBin::new("Takes").inside(at.interviews))
        .expect("the folder is created");

    panel.open(at.interviews);
    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    let painted = texts(&shapes);
    assert!(painted.iter().any(|text| text == "Takes"));
    assert!(painted.iter().any(|text| text == "Doc cut / Interviews"));

    // The collapse mark hides the child folder.
    let mark = text_center(&shapes, "-", 1).expect("the collapse mark was painted");
    frame(&ctx, &mut panel, &project, click_at(mark), Vec::new());
    assert!(panel.is_collapsed(at.interviews));
    // The rows are laid out before the click is handled, so the folder
    // disappears on the next frame.
    let (_, shapes) = frame(&ctx, &mut panel, &project, Vec::new(), Vec::new());
    assert!(!texts(&shapes).iter().any(|text| text == "Takes"));
}

/// The sample project's `Interviews` folder, which holds an item that is
/// online and carries its metadata.
fn fixture_interviews(project: &Project) -> BinId {
    project
        .root_bin
        .children
        .first()
        .expect("the sample project has folders")
        .id
}

#[test]
fn the_list_view_matches_its_snapshot() {
    // The root folder: the metadata columns over an offline item, which is
    // the one row the list draws differently.
    if !support::can_render() {
        return;
    }
    let project = support::fixture_project();
    let mut panel = MediaBinPanel::new();
    panel.mode = BinViewMode::List;
    let mut harness = support::panel_harness(|ui| {
        panel.ui(ui, &project);
    });
    harness.run();
    support::snapshot(&mut harness, "media_bin_list");
}

#[test]
fn the_grid_view_matches_its_snapshot() {
    // A folder deeper in, so the breadcrumb has something to say and the tile
    // has a duration and a resolution to draw.
    if !support::can_render() {
        return;
    }
    let project = support::fixture_project();
    let mut panel = MediaBinPanel::new();
    panel.mode = BinViewMode::Grid;
    panel.open(fixture_interviews(&project));
    let mut harness = support::panel_harness(|ui| {
        panel.ui(ui, &project);
    });
    harness.run();
    support::snapshot(&mut harness, "media_bin_grid");
}

#[test]
fn renaming_a_folder_through_the_harness_applies_and_undoes_a_command() {
    let project = support::fixture_project();
    let interviews = fixture_interviews(&project);
    let original = project.root_bin.children[0].name.clone();

    let mut panel = MediaBinPanel::new();
    panel.open(interviews);
    let mut harness = support::panel_harness_state(
        (panel, project, History::new()),
        |ui, (panel, project, history)| {
            // The panel mutates nothing itself: what it raises is applied
            // here, through the Command API, exactly as the app does it.
            for action in panel.ui(ui, project) {
                if let MediaBinAction::RenameBin { bin, name } = action {
                    history
                        .apply(project, RenameBin::new(bin, name))
                        .expect("the rename applies");
                }
            }
        },
    );
    harness.run();

    // The toolbar carries two fields: the new-folder name, then the rename.
    let field = harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .nth(1)
        .expect("the rename field is in the tree");
    field.click();
    harness.run();
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .nth(1)
        .expect("the rename field is still there")
        .type_text("Selects");
    harness.run();
    harness.get_by_label("Rename").click();
    harness.run();

    let (_, project, history) = harness.state_mut();
    assert_eq!(
        project.root_bin.children[0].name, "Selects",
        "the folder took the typed name"
    );
    history.undo(project).expect("the rename undoes");
    assert_eq!(project.root_bin.children[0].name, original);
}
