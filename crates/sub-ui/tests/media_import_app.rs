//! Importing through the assembled editor (TASK-136).
//!
//! `media_bin.rs` covers the panel raising an [`MediaBinAction::Import`] and
//! `media_import_fixtures.rs` covers the queue hashing and probing a real
//! file. This is the join between them, and the thing the v0.1.0 bug report
//! was about: the real [`SubordinateApp`], asked for an import exactly the way
//! a click on Import or a drop from the desktop asks for one, and judged by
//! what the project holds afterwards.
//!
//! Four things are proved that nothing below this level can prove:
//!
//! - the import actually happens. Before this the window logged "the bin asked
//!   for work this window does not host yet" and added nothing;
//! - it happens off the UI thread. The window keeps painting frames while the
//!   probe runs, and the bin shows the file as pending until it lands;
//! - a whole gesture is one entry in the undo stack, however many files it
//!   carried, and the MKV the report was about is one of them;
//! - a file that cannot be read surfaces as a [`SubError`] in the bin rather
//!   than as a log line.
//!
//! The test skips itself, saying why, when the fixtures have not been
//! generated or the machine enumerates no wgpu adapter — the same two reasons
//! the other assembled-window suites skip.

mod support;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use sub_core::SubError;
use sub_model::{Bin, MediaItem, MediaPath, Project};
use sub_time::Rational;
use sub_ui::media_bin::MediaBinAction;
use sub_ui::{AppOptions, SubordinateApp};

/// The constant-rate fixture: 125 frames at 25 fps of colour bars.
const MP4: &str = "bars_1080p_h264.mp4";

/// The Matroska fixture, because the user's report was an MKV: six seconds of
/// 720p, 30 fps for the first three and 60 fps for the last three.
const MKV: &str = "vfr_60_30.mkv";

/// How long the window is pumped for before the environment, not the code, is
/// called broken. Hashing and probing two files is seconds, not minutes.
const PATIENCE: Duration = Duration::from_mins(2);

/// The window this suite paints, in logical points, matching `app_engine.rs`.
const WINDOW_SIZE: egui::Vec2 = egui::vec2(1400.0, 900.0);

/// A fixture path, or `None` when the fixtures were never generated.
fn fixture(name: &str) -> Option<PathBuf> {
    let path = sub_test_support::try_fixture(name);
    if path.is_none() {
        eprintln!("skipping: no fixture {name}; run scripts/gen-fixtures.sh");
    }
    path
}

/// A saved project of this suite's own with `files` copied in beside it.
///
/// Saved, and not merely built, because a media path is stored relative to the
/// project file: a project that has never been written to disk has nothing for
/// an imported path to be relative to, and the editor refuses the import
/// rather than storing an absolute path.
fn project_with(name: &str, files: &[&str]) -> Option<(PathBuf, Vec<PathBuf>)> {
    let dir = std::env::temp_dir().join(format!("sub-ui-import-app-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("footage")).expect("a project folder");

    let mut copied = Vec::new();
    for file in files {
        let source = fixture(file)?;
        let target = dir.join("footage").join(file);
        std::fs::copy(&source, &target).expect("the fixture copies into the project folder");
        copied.push(target);
    }

    let path = dir.join("cut.sub");
    let project = Project::new("Import test");
    std::fs::write(
        &path,
        sub_model::json::to_json(&project).expect("the project serialises"),
    )
    .expect("the project writes");
    Some((path, copied))
}

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

/// The project as the engine holds it.
fn project_of(app: &SubordinateApp) -> std::sync::Arc<Project> {
    app.session().project_arc()
}

/// Pumps frames until `ready` answers true, and returns how many it took.
///
/// This is the UI thread doing exactly what it does in the editor: painting.
/// A probe that ran here would stop the count dead at one.
///
/// One `step` per turn rather than `run`, because a window with a job in
/// flight asks for the next frame itself and `run` refuses to paint more than
/// a few frames of a UI that keeps asking.
fn pump(
    harness: &mut Harness<'_, SubordinateApp>,
    mut ready: impl FnMut(&mut SubordinateApp) -> bool,
) -> u32 {
    let deadline = Instant::now() + PATIENCE;
    let mut frames = 0;
    while Instant::now() < deadline {
        harness.step();
        frames += 1;
        if ready(harness.state_mut()) {
            return frames;
        }
    }
    panic!("the window never finished the import within {PATIENCE:?}");
}

/// The imported item named `name`, if the project holds it.
fn imported<'a>(project: &'a Project, name: &str) -> Option<&'a MediaItem> {
    project.media.iter().find(|item| item.name == name)
}

/// The problems the bin is showing.
fn problems(app: &mut SubordinateApp) -> Vec<SubError> {
    app.media_bin().status().problems.clone()
}

#[test]
fn importing_from_the_bin_probes_off_the_ui_thread_and_lands_as_one_undo_step() {
    if !support::can_render() {
        return;
    }
    let Some((project_file, files)) = project_with("batch", &[MP4, MKV]) else {
        return;
    };
    let mut harness = app_harness(&project_file);
    support::run_settled(&mut harness);

    let root = project_of(harness.state()).root_bin.id;
    let before = project_of(harness.state()).media.len();
    assert_eq!(before, 0, "the project starts with no media");

    // Exactly what a click on Import raises once the file dialog has answered,
    // and exactly what a drop from the desktop raises: the bin builds this
    // action for both, so driving it here drives both paths.
    harness
        .state_mut()
        .apply_bin_action(MediaBinAction::Import {
            paths: files.clone(),
            bin: root,
        });

    // The probe has not happened yet, and the bin says so rather than freezing
    // the window: the pending rows are the files that were asked for.
    //
    // Read before the next frame rather than after one, because the frame
    // that collects a finished import is the same frame that clears its
    // pending row: a machine quick enough to probe both fixtures inside one
    // frame would otherwise look like a window that never showed them.
    let pending = harness.state_mut().media_bin().status().importing.clone();
    assert_eq!(
        pending.len(),
        files.len(),
        "the bin shows the files as pending while they are probed"
    );

    let frames = pump(&mut harness, |app| {
        project_of(app).media.len() == files.len()
    });

    assert!(
        frames > 1,
        "the window painted only one frame, so the probe ran on the UI thread"
    );
    assert!(
        problems(harness.state_mut()).is_empty(),
        "both fixtures import cleanly: {:?}",
        problems(harness.state_mut())
    );

    let project = project_of(harness.state());
    let bars = imported(&project, MP4).expect("the H.264 fixture imported");
    assert!(bars.path.is_external());
    assert_eq!(
        std::fs::canonicalize(bars.path.resolve(Path::new("unused"))).unwrap(),
        std::fs::canonicalize(&files[0]).unwrap()
    );
    assert!(bars.hash.is_some(), "the bytes were hashed");
    assert!(!bars.offline);
    assert_eq!(project.bin_of(bars.id), Some(root), "filed in the open bin");
    let info = bars.info.as_ref().expect("the file was probed");
    assert_eq!(
        info.duration
            .expect("a duration")
            .rescaled_to(Rational::FPS_25)
            .value(),
        125,
        "five seconds at 25 fps, exactly"
    );
    assert_eq!(
        (info.video[0].width, info.video[0].height),
        (1920, 1080),
        "the probe filled in the stream info the project file keeps"
    );

    // The MKV: the container the v0.1.0 report was about.
    let vfr = imported(&project, MKV).expect("the Matroska fixture imported");
    assert!(vfr.path.is_external());
    assert_eq!(
        std::fs::canonicalize(vfr.path.resolve(Path::new("unused"))).unwrap(),
        std::fs::canonicalize(&files[1]).unwrap()
    );
    assert!(vfr.hash.is_some());
    let vfr_info = vfr.info.as_ref().expect("the MKV was probed");
    assert_eq!(
        vfr_info
            .duration
            .expect("a duration")
            .rescaled_to(Rational::ONE)
            .value(),
        6,
        "six seconds, as the fixture catalogue declares"
    );
    assert_eq!(
        (vfr_info.video[0].width, vfr_info.video[0].height),
        (1280, 720)
    );
    drop(project);

    // One gesture, one entry in the undo stack, whatever it carried.
    let undone = harness.state_mut().session_mut().undo().expect("undo runs");
    assert!(undone, "the import is undoable");
    harness.step();
    assert!(
        project_of(harness.state()).media.is_empty(),
        "one press of undo takes the whole batch back out"
    );

    let _ = std::fs::remove_dir_all(project_file.parent().expect("a folder"));
}

#[test]
fn a_file_that_cannot_be_read_surfaces_a_sub_error_in_the_bin() {
    if !support::can_render() {
        return;
    }
    let Some((project_file, _)) = project_with("unreadable", &[]) else {
        return;
    };
    let dir = project_file.parent().expect("a folder").to_path_buf();
    let mut harness = app_harness(&project_file);
    support::run_settled(&mut harness);

    let root = project_of(harness.state()).root_bin.id;
    // Inside the project folder, so the relative-path rule is satisfied and
    // the refusal is about the file itself rather than about where it sits.
    let missing = dir.join("footage").join("never-copied.mkv");
    harness
        .state_mut()
        .apply_bin_action(MediaBinAction::Import {
            paths: vec![missing.clone()],
            bin: root,
        });

    // The bin says so on its own: no log line, no silence. The file is
    // never read on the UI thread, so the refusal arrives a frame or two
    // later, the same way a successful import does.
    pump(&mut harness, |app| {
        !app.media_bin().status().problems.is_empty()
    });

    let shown = problems(harness.state_mut());
    assert_eq!(shown.len(), 1, "one file asked for, one refusal shown");
    assert_eq!(
        shown[0]
            .details
            .get("path")
            .and_then(serde_json::Value::as_str),
        Some(missing.display().to_string().as_str()),
        "the refusal names the file that could not be read"
    );
    assert!(
        !shown[0].message.is_empty() && !shown[0].code.as_str().is_empty(),
        "a SubError with a stable code, which is what the bin draws"
    );
    assert!(
        project_of(harness.state()).media.is_empty(),
        "a file that cannot be read adds nothing to the project"
    );
    assert!(
        harness
            .state_mut()
            .media_bin()
            .status()
            .importing
            .is_empty(),
        "the pending row goes once the refusal is known"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A saved project whose one media item is offline, filed in a sub-folder.
///
/// The item is filed away from the root bin on purpose: the bin only draws a
/// per-item Relink button for what is in the open folder, so keeping it out of
/// sight leaves the dialog's own Relink button the only one on screen.
///
/// The file it should point at is real and sits in `footage/`, so a folder
/// search finds it by content hash.
fn offline_project(name: &str) -> Option<(PathBuf, sub_model::MediaId)> {
    let dir = std::env::temp_dir().join(format!("sub-ui-relink-app-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("footage")).expect("a project folder");

    let source = fixture(MKV)?;
    let moved = dir.join("footage").join(MKV);
    std::fs::copy(&source, &moved).expect("the fixture copies into the project folder");

    let mut project = Project::new("Relink test");
    // The project says the file is somewhere it is not, which is exactly what
    // a moved card or a re-copied drive leaves behind.
    let mut item = MediaItem::new(MediaPath::new("cards/card1/vfr_60_30.mkv").expect("a path"));
    item.offline = true;
    item.hash = Some(sub_model::ContentHash::of_file(&moved).expect("the file hashes"));
    let media = item.id;

    let mut rushes = Bin::new("Rushes");
    rushes.media.push(media);
    project.media.push(item);
    project.root_bin.children.push(rushes);

    let path = dir.join("cut.sub");
    std::fs::write(
        &path,
        sub_model::json::to_json(&project).expect("the project serialises"),
    )
    .expect("the project writes");
    Some((path, media))
}

#[test]
fn relinking_from_the_bin_searches_as_a_job_and_clears_the_offline_badge() {
    if !support::can_render() {
        return;
    }
    let Some((project_file, media)) = offline_project("search") else {
        return;
    };
    let dir = project_file.parent().expect("a folder").to_path_buf();
    let mut harness = app_harness(&project_file);
    support::run_settled(&mut harness);

    assert!(
        project_of(harness.state())
            .media_item(media)
            .expect("the offline item")
            .offline,
        "the project opens with the item offline"
    );

    // What the bin's Relink button raises. The dialog goes up on the next
    // frame; nothing has been searched or hashed yet.
    harness
        .state_mut()
        .apply_bin_action(MediaBinAction::Relink(media));
    harness.step();
    assert!(
        harness.state_mut().relink_dialog().is_open(),
        "the bin's Relink opens the dialog"
    );

    // The search is a job on the window's pool, the same one the imports run
    // on: the UI thread never walks the folder and never hashes a file.
    harness.state_mut().search_for_relink(&dir);
    assert!(
        harness.state_mut().relink_dialog().is_searching(),
        "the folder search runs as a job"
    );
    let frames = pump(&mut harness, |app| !app.relink_dialog().is_searching());
    assert!(
        frames > 1,
        "the window painted only one frame, so the search ran on the UI thread"
    );

    let found = harness.state_mut().relink_dialog().matches().len();
    assert_eq!(found, 1, "the search found the file by its content hash");

    // Accepting is a click, and what it applies goes through the session.
    harness.get_by_label("Relink").click();
    harness.step();
    harness.step();

    let project = project_of(harness.state());
    let item = project.media_item(media).expect("the item is still there");
    assert!(!item.offline, "the offline badge is cleared");
    assert_eq!(
        item.path.as_str(),
        "footage/vfr_60_30.mkv",
        "the item points at the file the search found"
    );
    assert_eq!(
        project.offline_media().count(),
        0,
        "nothing in the project is offline any more"
    );
    drop(project);

    // One search, one undo step.
    assert!(
        harness.state_mut().session_mut().undo().expect("undo runs"),
        "the relink is undoable"
    );
    harness.step();
    assert!(
        project_of(harness.state())
            .media_item(media)
            .expect("the item")
            .offline,
        "undo puts the item back offline"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unsaved_import_preview_and_first_save_keep_sources_and_pending_jobs() {
    if !support::can_render() {
        return;
    }
    let (Some(video), Some(audio)) = (fixture(MP4), fixture("tone_48k_stereo.wav")) else {
        return;
    };
    let mut harness = support::builder::<SubordinateApp>()
        .with_size(WINDOW_SIZE)
        .build_eframe(|cc| SubordinateApp::new(cc, AppOptions::default()).expect("editor starts"));
    support::run_settled(&mut harness);
    assert!(harness.state().project_file().is_none());
    let root = project_of(harness.state()).root_bin.id;
    harness
        .state_mut()
        .apply_bin_action(MediaBinAction::Import {
            paths: vec![video.clone()],
            bin: root,
        });
    pump(&mut harness, |app| app.session().project().media.len() == 1);
    assert!(problems(harness.state_mut()).is_empty());
    assert!(
        harness.state().project_file().is_none(),
        "import must not silently save the project"
    );
    let media = project_of(harness.state()).media[0].id;
    let from = harness
        .ctx
        .read_response(sub_ui::media_bin::drag_source_id(media))
        .unwrap()
        .interact_rect
        .center();
    let layout = harness.state_mut().timeline().layout().unwrap();
    let to = layout.content.left_top() + egui::vec2(0.1, 24.0);
    harness.hover_at(from);
    support::run_settled(&mut harness);
    harness.drag_at(from);
    support::run_settled(&mut harness);
    harness.hover_at(to);
    support::run_settled(&mut harness);
    harness.drop_at(to);
    support::run_settled(&mut harness);
    pump(&mut harness, |app| app.previews().stats().showing > 0);
    assert_viewer_preserves_compositor_colors(&mut harness);
    assert!(
        harness.state().project_file().is_none(),
        "preview works before Save"
    );

    // Save immediately after queueing a second import, before the UI collects it.
    harness
        .state_mut()
        .apply_bin_action(MediaBinAction::Import {
            paths: vec![audio.clone()],
            bin: root,
        });
    let save_dir = std::env::temp_dir().join(format!("sub-ui-first-save-{root}"));
    std::fs::create_dir_all(&save_dir).unwrap();
    let saved = save_dir.join("first.sub");
    harness.state_mut().save_project_as(&saved).unwrap();
    pump(&mut harness, |app| app.session().project().media.len() == 2);
    let before_undo = project_of(harness.state());
    harness.get_by_label("Edit").click();
    support::run_settled(&mut harness);
    harness.get_by_label_contains("Undo ").click();
    support::run_settled(&mut harness);
    assert_eq!(project_of(harness.state()).media.len(), 1);
    harness.get_by_label("Edit").click();
    support::run_settled(&mut harness);
    harness.get_by_label_contains("Redo ").click();
    support::run_settled(&mut harness);
    assert_eq!(*project_of(harness.state()), *before_undo);
    harness.state_mut().save_project().unwrap();
    let reopened = sub_model::json::from_json(&std::fs::read_to_string(&saved).unwrap()).unwrap();
    assert_eq!(reopened, *before_undo);
    for (item, original) in reopened.media.iter().zip([video, audio]) {
        assert!(item.path.is_external());
        assert_eq!(
            std::fs::canonicalize(item.absolute_path(&save_dir)).unwrap(),
            std::fs::canonicalize(original).unwrap()
        );
    }
    assert!(problems(harness.state_mut()).is_empty());
}

/// Compare the displayed video to the same compositor pixels used for export.
/// Sampling the broad SMPTE bars avoids interpolation and codec-edge noise.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn assert_viewer_preserves_compositor_colors(harness: &mut Harness<'_, SubordinateApp>) {
    let bounds = harness
        .output()
        .shapes
        .iter()
        .filter_map(|shape| {
            if let egui::Shape::Mesh(mesh) = &shape.shape
                && matches!(mesh.texture_id, egui::TextureId::User(_))
            {
                Some(mesh.calc_bounds())
            } else {
                None
            }
        })
        .max_by(|a, b| a.area().total_cmp(&b.area()))
        .expect("viewer texture is painted");
    let compositor = harness.state().compositor();
    let resolution = compositor.resolution();
    let reference = compositor.read_rgba();
    let rendered = harness.render().expect("the assembled editor renders");
    let mut checked = 0;
    for row in 1..20 {
        for column in 1..28 {
            let x = column as f32 / 28.0;
            let y = row as f32 / 20.0;
            let source_x = (resolution.width() as f32 * x) as u32;
            let source_y = (resolution.height() as f32 * y) as u32;
            let offset = ((source_y * resolution.width() + source_x) * 4) as usize;
            let expected = &reference[offset..offset + 3];
            if !expected.iter().any(|v| (64..224).contains(v)) {
                continue;
            }
            // Do not compare codec edges, text or the SMPTE noise patch.
            if [
                -4_i64,
                4,
                -4 * i64::from(resolution.width()),
                4 * i64::from(resolution.width()),
            ]
            .iter()
            .any(|delta| {
                let nearby =
                    (i64::from(source_y * resolution.width() + source_x) + delta) as usize * 4;
                reference[nearby..nearby + 3]
                    .iter()
                    .zip(expected)
                    .any(|(a, b)| a.abs_diff(*b) > 2)
            }) {
                continue;
            }
            let screen_x = (bounds.left() + bounds.width() * x) as u32;
            let screen_y = (bounds.top() + bounds.height() * y) as u32;
            let actual = rendered.get_pixel(screen_x, screen_y).0;
            for channel in 0..3 {
                assert!(
                    actual[channel].abs_diff(expected[channel]) <= 5,
                    "viewer at ({x},{y}) channel {channel}: displayed {}, compositor {}",
                    actual[channel],
                    expected[channel]
                );
            }
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "fixture must exercise midtones, not just black and white"
    );
}
