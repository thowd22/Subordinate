//! The dockable panel layout, painted headlessly.
//!
//! `egui::Context::run_ui` lays a frame out on the CPU with no window and no
//! GPU, so the dock can be drawn on any CI runner. What matters here is what
//! the user gets: every panel of docs/PLAN.md §5.7 has a tab, grouped tabs
//! show one panel at a time, a rearrangement survives a save and a reload,
//! and the menu item puts the default arrangement back.

mod support;

use eframe::egui;
use egui_dock::{NodePath, TabDestination, TabInsert};
use egui_kittest::kittest::Queryable;
use sub_ui::dock::{DockLayout, LAYOUT_FILE_NAME, Panel, layout_menu_ui};

/// Runs one headless frame drawing `body`.
fn frame<R>(ctx: &egui::Context, mut body: impl FnMut(&mut egui::Ui) -> R) -> R {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
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

/// Paints `layout` for one frame and reports which panel bodies were drawn.
fn painted_panels(layout: &mut DockLayout) -> Vec<Panel> {
    let ctx = egui::Context::default();
    // Two frames: the first lays the splits out, the second paints into them.
    let mut drawn = Vec::new();
    for _ in 0..2 {
        drawn.clear();
        frame(&ctx, |ui| {
            layout.ui(ui, |ui, panel| {
                ui.label(panel.title());
                drawn.push(panel);
            });
        });
    }
    drawn.sort_unstable();
    drawn
}

/// A scratch directory of this test's own.
fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("subordinate-dock-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn every_visible_panel_of_the_default_layout_is_drawn() {
    let mut layout = DockLayout::new();
    let drawn = painted_panels(&mut layout);
    // The inspector and export panels share a tab group, so exactly one of
    // them is on screen; the other three panels each have their own split.
    assert!(
        drawn.contains(&Panel::MediaBin)
            && drawn.contains(&Panel::Timeline)
            && drawn.contains(&Panel::Viewer),
        "the bin, timeline and viewer are each drawn: {drawn:?}"
    );
    assert_eq!(
        drawn
            .iter()
            .filter(|panel| matches!(panel, Panel::Inspector | Panel::Export))
            .count(),
        1,
        "the grouped tabs show one panel at a time: {drawn:?}"
    );
    assert_eq!(
        layout.panels().len(),
        Panel::ALL.len(),
        "and painting moves nothing"
    );
}

#[test]
fn a_panel_dragged_into_another_group_is_drawn_there() {
    let mut layout = DockLayout::new();
    let export = layout
        .state()
        .find_tab(&Panel::Export)
        .expect("the default layout has an export panel");
    let viewer = layout
        .state()
        .find_tab(&Panel::Viewer)
        .expect("and a viewer");
    // What a drag onto the viewer's tab bar does.
    layout.state_mut().move_tab(
        export,
        TabDestination::Node(
            NodePath {
                surface: viewer.surface,
                node: viewer.node,
            },
            TabInsert::Append,
        ),
    );

    let drawn = painted_panels(&mut layout);
    assert!(
        drawn.contains(&Panel::Export),
        "the moved panel is the active tab of its new group: {drawn:?}"
    );
    assert!(
        !drawn.contains(&Panel::Viewer),
        "and the viewer it landed on is now behind it: {drawn:?}"
    );
    assert_eq!(
        layout.panels().len(),
        Panel::ALL.len(),
        "no panel was lost in the move"
    );
}

#[test]
fn a_rearranged_layout_survives_a_save_and_a_reload() {
    let dir = scratch_dir("reload");
    let mut layout = DockLayout::new();
    let inspector = layout
        .state()
        .find_tab(&Panel::Inspector)
        .expect("the default layout has an inspector");
    let bin = layout
        .state()
        .find_tab(&Panel::MediaBin)
        .expect("and a media bin");
    layout.state_mut().move_tab(
        inspector,
        TabDestination::Node(
            NodePath {
                surface: bin.surface,
                node: bin.node,
            },
            TabInsert::Append,
        ),
    );
    let rearranged = layout.panels();
    assert!(
        layout.persist_to_dir(&dir).expect("the layout is written"),
        "a rearranged layout is written out"
    );

    let reloaded = DockLayout::load_from_dir(&dir);
    assert!(reloaded.problems.is_empty(), "{:?}", reloaded.problems);
    assert_eq!(
        reloaded.layout.panels(),
        rearranged,
        "the arrangement came back as it was left"
    );

    let mut reloaded = reloaded.layout;
    let drawn = painted_panels(&mut reloaded);
    assert!(
        drawn.contains(&Panel::Timeline),
        "and the reloaded layout paints: {drawn:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_menu_item_resets_the_arrangement() {
    let dir = scratch_dir("reset");
    let mut layout = DockLayout::new();
    let default_panels = layout.panels();
    let export = layout
        .state()
        .find_tab(&Panel::Export)
        .expect("the default layout has an export panel");
    let timeline = layout
        .state()
        .find_tab(&Panel::Timeline)
        .expect("and a timeline");
    layout.state_mut().move_tab(
        export,
        TabDestination::Node(
            NodePath {
                surface: timeline.surface,
                node: timeline.node,
            },
            TabInsert::Append,
        ),
    );
    layout.persist_to_dir(&dir).expect("the layout is written");
    assert_ne!(layout.panels(), default_panels, "the layout was rearranged");

    // The menu item, clicked for real. No adapter is needed: `AccessKit`
    // describes the tree egui laid out, so the interaction half of the
    // harness runs anywhere.
    let mut harness =
        support::panel_harness_state((layout, false), |ui, (layout, reset): &mut (_, _)| {
            *reset |= layout_menu_ui(ui, layout);
        });
    harness.run();
    let (layout, reset) = harness.state();
    assert!(!*reset, "nothing has been clicked yet");
    assert_ne!(
        layout.panels(),
        default_panels,
        "and nothing moved on its own"
    );

    harness.get_by_label("Reset layout").click();
    harness.run();
    let (layout, reset) = harness.state();
    assert!(*reset, "the menu item reports the reset");
    assert_eq!(
        layout.panels(),
        default_panels,
        "clicking it restores the default arrangement"
    );

    let (mut layout, _) = harness.into_state();
    assert!(
        layout.is_dirty(),
        "and the reset layout differs from the saved one"
    );
    assert!(
        layout.persist_to_dir(&dir).expect("the reset layout saves"),
        "so the next save writes it"
    );
    assert!(
        dir.join(LAYOUT_FILE_NAME).is_file(),
        "the layout file is where the config directory says"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
