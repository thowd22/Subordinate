//! The sequence tab strip, driven headlessly.
//!
//! `egui::Context::run_ui` lays out and paints a frame on the CPU with no
//! window and no GPU, which is what lets the strip be exercised on a CI
//! runner. What the tests care about is what the strip remembers: switching
//! tabs must put the timeline and the viewer back exactly where they were,
//! and the last tab must refuse to close.

use eframe::egui;
use sub_model::media::{StreamInfo, VideoStream};
use sub_model::sequence::{Resolution, SequenceSettings};
use sub_model::{
    Clip, ColorTags, MediaItem, MediaPath, Project, Sequence, Track, TrackItem, TrackKind,
};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::sequence_tabs::{NewSequenceDialog, SequenceTabAction, SequenceTabs};
use sub_ui::timeline::ZoomLevel;
use sub_ui::timeline_panel::TimelinePanel;
use sub_ui::viewer::ViewerState;

/// A project with two sequences at different timebases, each holding one
/// four-hundred-frame clip.
fn project() -> Project {
    let mut project = Project::new("tabs");
    let mut item = MediaItem::new(MediaPath::new("media/take.mp4").expect("valid path"));
    item.info = Some(StreamInfo {
        duration: Some(RationalTime::new(10_000, Rational::FPS_24)),
        video: vec![VideoStream {
            width: 1920,
            height: 1080,
            frame_rate: Rational::FPS_24,
            sample_aspect: Rational::ONE,
            color: ColorTags::REC709,
        }],
        audio: Vec::new(),
    });
    let media = item.id;
    project.media.push(item);

    for (name, rate) in [("Main", Rational::FPS_24), ("Titles", Rational::FPS_25)] {
        let settings = SequenceSettings::new(Resolution::HD_1080, rate, 48_000, ColorTags::REC709)
            .expect("valid settings");
        let mut sequence = Sequence::new(name, settings);
        let mut track = Track::new("V1", TrackKind::Video);
        let source = TimeRange::new(RationalTime::zero(rate), RationalTime::new(400, rate))
            .expect("valid source range");
        track
            .items
            .push(TrackItem::Clip(Clip::new("take", media, source)));
        sequence.tracks.push(track);
        project.sequences.push(sequence);
    }
    project
}

/// Asserts a lane scroll, which is a point measurement rather than a timeline
/// one and so is the one value here that is a float.
#[track_caller]
fn assert_lane_scroll(actual: f32, expected: f32, label: &str) {
    assert!(
        (actual - expected).abs() < 1.0 / 1024.0,
        "{label}: lane scroll is {actual}, expected {expected}"
    );
}

/// Runs one headless frame with the strip in it, returning what it asked for.
fn frame(
    ctx: &egui::Context,
    tabs: &mut SequenceTabs,
    sequences: &[Sequence],
) -> Option<SequenceTabAction> {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1200.0, 600.0),
        )),
        ..Default::default()
    };
    let mut action = None;
    let mut output = ctx.run_ui(input, |ui| {
        action = tabs.ui(ui, sequences);
    });
    let _ = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
    output.textures_delta.clear();
    action
}

#[test]
fn the_strip_paints_a_tab_per_sequence_and_asks_for_nothing_on_its_own() {
    let project = project();
    let ctx = egui::Context::default();
    let mut tabs = SequenceTabs::new();
    assert_eq!(tabs.sync(&project.sequences), Some(project.sequences[0].id));

    // Two frames: egui needs a second pass before its widgets have their real
    // sizes, and neither pass may invent an edit out of nothing.
    for _ in 0..2 {
        assert_eq!(frame(&ctx, &mut tabs, &project.sequences), None);
    }
    assert_eq!(tabs.active(), Some(project.sequences[0].id));
    assert_eq!(tabs.renaming(), None);
}

#[test]
fn switching_sequences_preserves_zoom_scroll_and_playhead() {
    let project = project();
    let main = &project.sequences[0];
    let titles = &project.sequences[1];

    let mut tabs = SequenceTabs::new();
    tabs.sync(&project.sequences);
    let mut panel = TimelinePanel::new(main.settings.frame_rate);
    let mut viewer = ViewerState::for_sequence(main);
    panel.view_mut().set_width_px(1000);

    // Look at the first sequence somewhere specific.
    let main_zoom = ZoomLevel::ONE.scaled(8, 1);
    panel.view_mut().set_zoom(main_zoom);
    panel.view_mut().set_scroll_px(640);
    panel.restore_lane_scroll(24.0);
    assert!(viewer.seek_to_frame(96));

    tabs.switch_to(&project.sequences, titles.id, &mut panel, &mut viewer)
        .expect("the second sequence exists");
    assert_eq!(tabs.active(), Some(titles.id));
    assert_eq!(
        viewer.rate(),
        titles.settings.frame_rate,
        "the viewer takes on the new sequence's timebase"
    );
    assert_eq!(
        panel.view().zoom(),
        ZoomLevel::ONE,
        "a fresh tab starts fresh"
    );
    assert_eq!(panel.view().scroll_px(), 0);
    assert_lane_scroll(panel.lane_scroll_px(), 0.0, "a fresh tab");
    assert_eq!(viewer.playhead_frame(), 0);

    // Move about on the second sequence too, then go back.
    let titles_zoom = ZoomLevel::ONE.scaled(1, 4);
    panel.view_mut().set_zoom(titles_zoom);
    panel.view_mut().set_scroll_px(11);
    panel.restore_lane_scroll(7.5);
    assert!(viewer.seek_to_frame(120));

    tabs.switch_to(&project.sequences, main.id, &mut panel, &mut viewer)
        .expect("the first sequence exists");
    assert_eq!(panel.view().rate(), main.settings.frame_rate);
    assert_eq!(panel.view().zoom(), main_zoom, "zoom came back");
    assert_eq!(panel.view().scroll_px(), 640, "scroll came back");
    assert_lane_scroll(panel.lane_scroll_px(), 24.0, "lane scroll came back");
    assert_eq!(
        viewer.playhead(),
        RationalTime::new(96, main.settings.frame_rate),
        "the playhead came back"
    );
    assert_eq!(
        panel.view().width_px(),
        1000,
        "the viewport width belongs to the window, not the tab"
    );

    // And the second sequence still remembers its own position.
    tabs.switch_to(&project.sequences, titles.id, &mut panel, &mut viewer)
        .expect("the second sequence exists");
    assert_eq!(panel.view().zoom(), titles_zoom);
    assert_eq!(panel.view().scroll_px(), 11);
    assert_lane_scroll(panel.lane_scroll_px(), 7.5, "the second tab kept its own");
    assert_eq!(
        viewer.playhead(),
        RationalTime::new(120, titles.settings.frame_rate)
    );
}

#[test]
fn a_deleted_sequence_hands_the_tab_over_and_the_last_one_cannot_be_deleted() {
    let mut project = project();
    let main = project.sequences[0].id;
    let titles = project.sequences[1].id;
    let mut tabs = SequenceTabs::new();
    tabs.sync(&project.sequences);
    let mut panel = TimelinePanel::new(Rational::FPS_24);
    let mut viewer = ViewerState::new(Rational::FPS_24);
    tabs.switch_to(&project.sequences, titles, &mut panel, &mut viewer)
        .expect("known sequence");

    // Both are deletable while there are two of them.
    SequenceTabs::check_delete(&project.sequences, titles).expect("two sequences");
    project.sequences.retain(|sequence| sequence.id != titles);

    assert_eq!(
        tabs.sync(&project.sequences),
        Some(main),
        "the tab moves to what is left"
    );
    assert!(
        tabs.remembered(titles).is_none(),
        "and forgets the deleted one"
    );

    let err = SequenceTabs::check_delete(&project.sequences, main).unwrap_err();
    assert_eq!(err.code, sub_ui::codes::LAST_SEQUENCE);
}

#[test]
fn the_new_sequence_dialog_yields_a_create_action() {
    let project = project();
    let ctx = egui::Context::default();
    let mut dialog = NewSequenceDialog::new();
    dialog.open_with("Sequence 3");
    dialog.frame_rate = Rational::FPS_29_97;
    dialog.sample_rate = 44_100;

    // Painting the dialog must not create anything by itself.
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(600.0, 400.0),
        )),
        ..Default::default()
    };
    let mut action = None;
    let mut output = ctx.run_ui(input, |ui| {
        action = dialog.body_ui(ui, "Sequence 3");
    });
    let _ = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
    output.textures_delta.clear();
    assert_eq!(action, None);

    let settings = dialog.settings().expect("valid settings");
    assert_eq!(settings.frame_rate, Rational::FPS_29_97);
    assert_eq!(settings.sample_rate, 44_100);
    assert_eq!(settings.resolution, Resolution::HD_1080);
    assert_eq!(
        dialog.effective_name(&sub_ui::default_sequence_name(&project.sequences)),
        "Sequence 3"
    );
}
