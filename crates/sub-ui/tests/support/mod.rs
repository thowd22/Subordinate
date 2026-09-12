//! The shared UI test harness: `egui_kittest` around any panel.
//!
//! `egui_kittest` runs a real egui pass, renders it through wgpu on whatever
//! adapter the machine offers (a software one on CI), and exposes the
//! resulting tree over `AccessKit` so a test can click a button by its label.
//! Two things come out of that: PNG snapshots that are diffed against
//! committed references, and interaction tests that drive a panel the way a
//! user would and then assert on the model rather than on pixels.
//!
//! Every test that paints goes through [`builder`] so the frame is the same
//! shape everywhere: 800x600 logical pixels at one physical pixel per point,
//! the dark theme, and the wgpu renderer. Snapshots land in
//! `crates/sub-ui/tests/snapshots/`; the tolerances and the update procedure
//! are configured in `kittest.toml` at the workspace root and documented in
//! `docs/DEVELOPMENT.md`.
//!
//! Machines with no wgpu adapter at all (a container with no ICD) cannot
//! render. [`gpu_available`] answers that once per process, and the
//! `skip_without_gpu!` macro turns it into a test that reports and passes
//! rather than failing the build on an environment problem. Interaction tests
//! that never call [`snapshot`] need no adapter and always run.
//!
//! Panels are painted over the committed sample project
//! (`crates/sub-model/tests/fixtures/sample-project.sub`), so every snapshot
//! shows the same realistic two-sequence, three-track edit and a change to
//! that fixture shows up as a visible diff.

// Each integration test file pulls this module in with `mod support;` and uses
// only the part of it that it needs.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use sub_model::{Project, Sequence};
use sub_ui::SubordinateApp;

/// The size every panel is painted at, in logical points.
///
/// Snapshot PNGs are committed, so the frame is kept small on purpose: 800x600
/// at one pixel per point is a few tens of kilobytes of flat UI colour.
pub const PANEL_SIZE: egui::Vec2 = egui::vec2(800.0, 600.0);

/// The committed sample project, as it lives in the `sub-model` fixtures.
pub fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../sub-model/tests/fixtures/sample-project.sub")
}

/// Loads the committed sample project.
///
/// # Panics
///
/// Panics when the fixture is missing or does not parse, which is a broken
/// checkout rather than a UI failure.
pub fn fixture_project() -> Project {
    let path = fixture_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("could not read {}: {err}", path.display()));
    sub_model::json::from_json(&text)
        .unwrap_or_else(|err| panic!("could not load {}: {err}", path.display()))
}

/// The sequence a panel test shows by default: the fixture's first one.
///
/// # Panics
///
/// Panics when the fixture has no sequences.
pub fn fixture_sequence(project: &Project) -> &Sequence {
    project
        .sequences
        .first()
        .expect("the sample project has sequences")
}

/// Whether this machine offers a wgpu adapter at all.
///
/// The question is asked the way the editor asks it, through
/// [`sub_render::RenderContext::headless`], and answered once per process:
/// enumerating adapters walks every installed driver.
pub fn gpu_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| match sub_render::RenderContext::headless() {
        Ok(_) => true,
        Err(sub_render::RenderError::NoAdapter { .. }) => false,
        Err(error) => {
            eprintln!("no usable adapter: {error}");
            false
        }
    })
}

/// Whether a snapshot can be taken here, reporting the reason when it cannot.
///
/// A test that renders opens with `if !support::can_render() { return; }`, so
/// a machine with no ICD reports and passes rather than failing the build on
/// an environment problem. Only rendering needs an adapter; a test that merely
/// drives input and asserts on the model should not ask.
pub fn can_render() -> bool {
    if gpu_available() {
        return true;
    }
    eprintln!("skipping: this machine enumerates no wgpu adapter");
    false
}

/// A harness builder with the shared frame settings applied.
///
/// Callers may still override the size (a tall panel needs a taller frame),
/// but everything else should stay as it is so snapshots stay comparable.
pub fn builder<State>() -> HarnessBuilder<State> {
    Harness::builder()
        .with_size(PANEL_SIZE)
        .with_pixels_per_point(1.0)
        .with_theme(egui::Theme::Dark)
        .wgpu()
}

/// A harness painting `panel` into the standard frame.
pub fn panel_harness<'a>(panel: impl FnMut(&mut egui::Ui) + 'a) -> Harness<'a> {
    builder().build_ui(panel)
}

/// A harness painting `panel` over some state the test owns.
///
/// The state is what an interaction test asserts on afterwards: the panel's
/// own struct, the project, the action it returned.
pub fn panel_harness_state<'a, State>(
    state: State,
    panel: impl FnMut(&mut egui::Ui, &mut State) + 'a,
) -> Harness<'a, State> {
    builder().build_ui_state(panel, state)
}

/// Renders the harness and compares it against `tests/snapshots/<name>.png`.
///
/// # Panics
///
/// Panics when the render differs from the committed snapshot beyond the
/// tolerance in `kittest.toml`, writing `<name>.new.png` and `<name>.diff.png`
/// next to it. Re-record with `UPDATE_SNAPSHOTS=1 cargo test -p sub-ui`.
pub fn snapshot<State>(harness: &mut Harness<'_, State>, name: &str) {
    harness.snapshot(name);
}

/// How long [`run_settled`] waits for the preview decoders to catch up.
///
/// Opening a decoder means building a PTS index and starting a pipeline on a
/// worker, which is tens of milliseconds on a warm machine and seconds on a
/// cold CI runner decoding in software.
pub const PREVIEW_PATIENCE: Duration = Duration::from_secs(45);

/// Paints the assembled editor until its preview has settled, then runs it to
/// a stop.
///
/// `Harness::run` on its own is the wrong tool for the whole window. While a
/// preview decoder is opening, or a picture for the frame the playhead is on
/// is still on its way, the window asks for the next frame itself (see
/// `PREVIEW_POLL_INTERVAL` in `sub_ui::app`) — and `run` refuses to paint more
/// than a handful of frames of a UI that keeps asking, which is the
/// `exceeded max_steps` panic.
///
/// So this waits the way `viewer_decode` and `media_import_app` wait: a
/// bounded loop of real painted frames until
/// [`sub_ui::preview::PreviewService::settled`] says every layer under the
/// playhead is showing the frame it is on, and only then lets the layout come
/// to rest. A window over a project with no media settles on its first frame,
/// so this is `run` with a wait in front of it.
///
/// # Panics
///
/// Panics when the preview has not settled within [`PREVIEW_PATIENCE`],
/// reporting what it was still waiting for.
pub fn run_settled(harness: &mut Harness<'_, SubordinateApp>) {
    let deadline = Instant::now() + PREVIEW_PATIENCE;
    loop {
        harness.step();
        if harness.state().previews().settled() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the preview did not settle within {PREVIEW_PATIENCE:?}: {:?}, failures {:?}",
            harness.state().previews().stats(),
            harness.state().previews().failures(),
        );
        // A painted frame costs the harness no wall time and the decoders are
        // on workers of their own, so without this the loop would spend its
        // whole budget before a worker had been scheduled once. The window
        // itself never sleeps: it asks for a repaint and returns.
        std::thread::sleep(Duration::from_millis(5));
    }
    // Settled, so nothing here is asking for another frame on the preview's
    // account; `run_ok` lets whatever else is animating finish without turning
    // a slow runner into a failure.
    let _ = harness.run_ok();
}
