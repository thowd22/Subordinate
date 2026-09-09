//! Painting the timeline panel headlessly: what it emits, and what it costs.
//!
//! `egui::Context::run_ui` does everything the editor does on the CPU each frame
//! — input, layout, painting and tessellation — and needs no window and no
//! GPU, so the panel's per-frame cost can be measured on any machine,
//! including a CI runner with no display. What is left out is the GPU upload
//! and draw, which for a few thousand flat triangles is not the bottleneck;
//! the CPU frame is.

use std::time::{Duration, Instant};

use eframe::egui;
use sub_model::media::{StreamInfo, VideoStream};
use sub_model::sequence::SequenceSettings;
use sub_model::{
    Clip, ColorTags, MediaItem, MediaPath, Project, Sequence, Track, TrackItem, TrackKind,
};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::timeline::ZoomLevel;
use sub_ui::timeline_panel::TimelinePanel;

/// The sequence timebase every test here uses.
const RATE: Rational = Rational::FPS_24;

/// How many clips the performance test paints.
const CLIPS: usize = 500;

/// The budget for one painted frame: 60 frames per second.
const FRAME_BUDGET: Duration = Duration::from_micros(16_666);

/// A project whose sequence holds `clips` clips spread over `tracks` tracks.
fn scene(clips: usize, tracks: usize) -> (Project, Sequence) {
    let mut project = Project::new("timeline");
    let mut item = MediaItem::new(MediaPath::new("media/take.mp4").expect("valid path"));
    item.info = Some(StreamInfo {
        duration: Some(RationalTime::new(10_000, RATE)),
        video: vec![VideoStream {
            width: 1920,
            height: 1080,
            frame_rate: RATE,
            sample_aspect: Rational::ONE,
            color: ColorTags::REC709,
        }],
        audio: Vec::new(),
    });
    let media = item.id;
    project.media.push(item);

    let mut sequence = Sequence::new("edit", SequenceSettings::default());
    for track_index in 0..tracks {
        let mut track = Track::new(format!("V{}", track_index + 1), TrackKind::Video);
        for clip_index in 0..clips.div_ceil(tracks) {
            // Head-trimmed and tail-trimmed, so the trim indicators are painted
            // too; the clips tile the track back to back.
            let source = TimeRange::new(
                RationalTime::new(12, RATE),
                RationalTime::new(36 + i64::try_from(clip_index % 5).unwrap_or(0), RATE),
            )
            .expect("valid source range");
            track.items.push(TrackItem::Clip(Clip::new(
                format!("take {track_index}-{clip_index}"),
                media,
                source,
            )));
        }
        sequence.tracks.push(track);
    }
    (project, sequence)
}

/// Paints `panel` once into a fresh egui pass, returning what that cost.
fn paint(ctx: &egui::Context, panel: &mut TimelinePanel, project: &Project, sequence: &Sequence) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 700.0),
        )),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        panel.sync(sequence, 1);
        panel.ui(ui, project, sequence);
    });
    // Tessellation is part of the frame the editor pays for, so measure it.
    let _ = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
    // With no GPU here there is nowhere to apply the font atlas deltas to; a
    // real backend uploads them instead of dropping them.
    output.textures_delta.clear();
}

#[test]
fn a_painted_frame_only_touches_the_visible_clips() {
    let (project, sequence) = scene(CLIPS, 1);
    let mut panel = TimelinePanel::new(RATE);
    panel.view_mut().set_zoom(ZoomLevel::ONE);
    let ctx = egui::Context::default();
    paint(&ctx, &mut panel, &project, &sequence);

    assert_eq!(panel.layouts().len(), 1, "one index per track");
    assert_eq!(panel.layouts()[0].len(), CLIPS);
    let visible = panel.view().visible_clips(&panel.layouts()[0]);
    assert!(!visible.is_empty(), "the first clips are on screen");
    assert!(
        visible.len() < CLIPS / 4,
        "the panel is virtualised: {} of {CLIPS} clips are in the viewport",
        visible.len()
    );
    assert!(
        panel.view().width_px() > 1000,
        "the lanes take the width left by the header column"
    );
}

#[test]
fn five_hundred_clips_paint_inside_a_sixty_hertz_frame() {
    // Zoomed far enough out that all 500 clips are inside the viewport at
    // once, which is the worst case for the painter.
    let (project, sequence) = scene(CLIPS, 4);
    let mut panel = TimelinePanel::new(RATE);
    let ctx = egui::Context::default();
    panel.sync(&sequence, 1);
    let mut fit = *panel.view();
    fit.set_width_px(1468);
    fit.fit(panel.content_duration());
    *panel.view_mut() = fit;

    let whole = measure(
        &ctx,
        &mut panel,
        &project,
        &sequence,
        "all 500 clips in view",
    );

    // And zoomed in far enough that the clips on screen are wide enough to
    // carry their names, which is what makes a frame expensive.
    let mut close = *panel.view();
    close.set_zoom(panel.view().zoom().scaled(16, 1));
    *panel.view_mut() = close;
    let named = measure(&ctx, &mut panel, &project, &sequence, "clip names painted");

    for (label, best) in [("zoomed out", whole), ("zoomed in", named)] {
        assert!(
            best <= FRAME_BUDGET,
            "painting {CLIPS} clips {label} took {best:?}, over the {FRAME_BUDGET:?} 60 fps budget"
        );
    }
}

/// Paints thirty frames and returns the fastest.
///
/// The fastest frame is the machine's honest cost; the median and worst are
/// printed so a loaded machine's noise shows up in the log rather than
/// failing the run.
fn measure(
    ctx: &egui::Context,
    panel: &mut TimelinePanel,
    project: &Project,
    sequence: &Sequence,
    label: &str,
) -> Duration {
    for _ in 0..5 {
        paint(ctx, panel, project, sequence);
    }
    let mut timings = Vec::with_capacity(30);
    for _ in 0..30 {
        let started = Instant::now();
        paint(ctx, panel, project, sequence);
        timings.push(started.elapsed());
    }
    timings.sort_unstable();
    println!(
        "{label}: best {:?}, median {:?}, worst {:?}",
        timings[0],
        timings[timings.len() / 2],
        timings[timings.len() - 1]
    );
    timings[0]
}
