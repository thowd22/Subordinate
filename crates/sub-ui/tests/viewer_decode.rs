//! The viewer shows the decoded frame the playhead is on (TASK-144).
//!
//! Every other viewer suite here paints the panel over a texture the test
//! made up. This one builds the real [`SubordinateApp`] through `egui_kittest`'s
//! eframe harness over a project cut from a generated fixture, scrubs it from
//! the keyboard the way a user does, and then asks two questions of the picture
//! the window is actually showing:
//!
//! * **Does it read as frame N?** The fixture is colour bars with a timecode
//!   painted across the bottom by `timeoverlay`, so the frame the viewer is on
//!   can be read off the canvas. The reading is learnt the way
//!   `sub-media`'s `seek_fixtures` learns it — one thresholded burn-in window
//!   per timecode second, decoded from the file — and the canvas's window is
//!   matched against those, so an off-by-one-second frame fails rather than
//!   being waved through by a tolerance.
//! * **Is it that frame's picture?** The same frame is decoded straight out of
//!   the fixture, uploaded and composited through a second compositor on the
//!   same device, and the two canvases are compared. That is a whole-picture
//!   comparison against a reference decoded from the file, not a check that
//!   something non-black was drawn.
//!
//! The scrub itself is the interaction: arrow keys through the window's own
//! keymap, forwards and then backwards — the backward half is the one the
//! TASK-133 GOP cache serves — with the picture asserted at each landing.
//!
//! The suite reports and passes when the fixtures have not been generated or
//! the machine enumerates no wgpu adapter, so `cargo test` works on a fresh
//! checkout.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Key, Modifiers};
use eframe::wgpu;
use egui_kittest::Harness;
use sub_media::{Decoder, DecoderOptions, FrameFormat, PtsIndex, StreamSelection, VideoFrame};
use sub_model::{
    Clip, ColorTags, MediaItem, MediaPath, Project, Resolution, Sequence, SequenceSettings, Track,
    TrackKind,
};
use sub_render::{
    Compositor, Nv12Converter, Nv12Geometry, RenderContext, ResolvedClip, SourceFrame,
};
use sub_time::{Rational, RationalTime, TimeRange, Timecode, TimecodeRate};
use sub_ui::{AppOptions, SubordinateApp};

/// The fixture the project is cut from: 1080p colour bars at 25 fps with a
/// burnt-in timecode, five seconds long.
const FIXTURE: &str = "bars_1080p_h264.mp4";

/// How many pictures that fixture holds.
const FRAME_COUNT: usize = 125;

/// Luma above which a burn-in pixel counts as ink, as `seek_fixtures` reads it.
const INK: u8 = 160;

/// The window this suite drives, in logical points. The whole editor, as
/// `app_engine` paints it.
const WINDOW_SIZE: egui::Vec2 = egui::vec2(1400.0, 900.0);

/// How many frames the harness may paint while waiting for a decode.
///
/// A software decode of a 1080p keyframe seek is tens of milliseconds and the
/// window repaints every 8 ms, so this is seconds of slack rather than a
/// schedule.
const SETTLE_FRAMES: usize = 400;

/// The fixture's timebase.
fn rate() -> Rational {
    Rational::FPS_25
}

/// The fixture's timecode rate: 25 fps never drops frames.
fn timecode_rate() -> TimecodeRate {
    TimecodeRate::non_drop(rate()).expect("25 fps is a valid timecode rate")
}

/// The timecode the fixture's frame `index` carries.
fn timecode(index: usize) -> Timecode {
    Timecode::from_frame_number(
        i64::try_from(index).expect("a small frame number"),
        timecode_rate(),
    )
}

/// The generated fixture, or `None` when it has not been generated.
fn fixture() -> Option<PathBuf> {
    match sub_test_support::fixture(FIXTURE) {
        Ok(path) => Some(path),
        Err(error) => {
            eprintln!("skipping: {error}");
            None
        }
    }
}

/// The decoder options the preview uses, so the reference is decoded exactly
/// as the viewer's own pipeline decodes.
fn decoder_options() -> DecoderOptions {
    DecoderOptions {
        streams: StreamSelection::Video,
        format: FrameFormat::Nv12,
        ..DecoderOptions::default()
    }
}

/// A folder of this test's own, emptied first so a rerun starts clean.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-ui-viewer-decode-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary folder");
    dir
}

/// Writes a one-clip project over `fixture` into a folder of its own and
/// returns the project file.
///
/// The fixture is copied in beside the project so the clip's media path is
/// relative, which is what a real project holds and what the preview resolves
/// against the project's own folder.
fn project_over(fixture: &Path, name: &str, first_picture: RationalTime) -> PathBuf {
    let dir = temp_dir(name);
    let media_name = "bars.mp4";
    std::fs::copy(fixture, dir.join(media_name)).expect("the fixture copies");

    let mut item = MediaItem::new(MediaPath::new(media_name).expect("a valid relative path"));
    "Bars".clone_into(&mut item.name);
    let media_id = item.id;

    let settings = SequenceSettings::new(
        Resolution::new(1920, 1080).expect("a valid canvas"),
        rate(),
        48_000,
        ColorTags::default(),
    )
    .expect("valid sequence settings");
    let mut sequence = Sequence::new("Main", settings);
    let mut track = Track::new("V1", TrackKind::Video);
    // The source range starts at the file's first picture rather than at a
    // bare zero. These fixtures' timestamps begin 80 ms in, and a clip's
    // source time is the media's own timeline — the same timeline
    // `Decoder::seek_to` and the export path are given — so a clip that means
    // "from the top of the file" starts where the file's pictures start. An
    // importer builds it from the probe; here it comes from the PTS index.
    let span = TimeRange::new(
        first_picture,
        RationalTime::new(i64::try_from(FRAME_COUNT).expect("a small count"), rate()),
    )
    .expect("a valid source range");
    let mut clip = Clip::new("Bars", media_id, span);
    "Bars".clone_into(&mut clip.name);
    track.items.push(sub_model::TrackItem::Clip(clip));
    sequence.tracks.push(track);

    let mut project = Project::new("Decode");
    project.root_bin.media.push(media_id);
    project.media.push(item);
    project.sequences.push(sequence);

    let path = dir.join("decode.sub");
    std::fs::write(
        &path,
        sub_model::json::to_json(&project).expect("the project serialises"),
    )
    .expect("the project writes");
    path
}

/// The thresholded burn-in window of a picture, as `seek_fixtures` reads it:
/// the lower band of the frame, left of the noise field under the counter.
///
/// The window is taken from a luma plane, whatever it came out of — the
/// decoder's NV12 or the compositor's RGBA — so a decoded frame and a canvas
/// can be read the same way.
struct BurnIn {
    ink: Vec<bool>,
}

impl BurnIn {
    /// Thresholds the window out of a `width` x `height` luma raster.
    fn of_luma(luma: &[u8], width: usize, height: usize, stride: usize) -> Self {
        let (x0, x1) = (width * 3 / 8, width * 3 / 4);
        let (y0, y1) = (height * 7 / 10, height * 9 / 10);
        let mut ink = Vec::with_capacity((x1 - x0) * (y1 - y0));
        for y in y0..y1 {
            for x in x0..x1 {
                ink.push(luma[y * stride + x] > INK);
            }
        }
        Self { ink }
    }

    /// The window of a decoded NV12 picture.
    fn of_frame(frame: &VideoFrame) -> Self {
        let stride = frame.plane_stride(0).expect("a luma stride") as usize;
        let luma = frame.plane_data(0).expect("a luma plane");
        Self::of_luma(
            luma,
            frame.width() as usize,
            frame.height() as usize,
            stride,
        )
    }

    /// The window of a composited RGBA canvas.
    ///
    /// The luma is recovered with the Rec. 601 weights the conversion shader
    /// inverts. Exactness does not matter here: what is read is which pixels
    /// carry ink, and the burn-in is white text on colour bars.
    fn of_rgba(rgba: &[u8], width: usize, height: usize) -> Self {
        let mut luma = Vec::with_capacity(width * height);
        for pixel in rgba.chunks_exact(4).take(width * height) {
            let value =
                (u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29)
                    >> 8;
            luma.push(u8::try_from(value.min(255)).unwrap_or(255));
        }
        Self::of_luma(&luma, width, height, width)
    }

    /// How many pixels two windows disagree on.
    fn differing(&self, other: &Self) -> usize {
        assert_eq!(self.ink.len(), other.ink.len(), "windows of one geometry");
        self.ink
            .iter()
            .zip(&other.ink)
            .filter(|(a, b)| a != b)
            .count()
    }

    /// How far two windows may disagree and still show the same digit.
    ///
    /// Wider than `seek_fixtures`' thousandth, because these two windows did
    /// not come out of the same pipeline: one is the decoder's luma plane and
    /// the other is that plane through the conversion shader, the compositor's
    /// sampler and back out of a readback. A hundredth of the window is still
    /// far tighter than the gap between two digits, which is checked below.
    fn same_digit(&self) -> usize {
        self.ink.len() / 100
    }

    /// How far apart two windows must be to show different digits.
    ///
    /// Twice [`Self::same_digit`], which is the property the reading actually
    /// needs: a canvas within the tolerance of the second it shows is then
    /// still at least that tolerance away from every other second, so the
    /// nearest match cannot be the wrong one. A flat fraction of the window
    /// instead measures the *font* — how many pixels apart `timeoverlay` draws
    /// a 2 from a 3 — which is not the same on every platform (Windows: 3769
    /// of 155,520, a whisker under the window's fortieth).
    fn different_digit(&self) -> usize {
        self.same_digit() * 2
    }
}

/// The fixture decoded from the start: what its burn-in means, second by
/// second.
struct BurnInKey {
    seconds: BTreeMap<u32, BurnIn>,
}

impl BurnInKey {
    /// Learns one burn-in window per timecode second by decoding the file.
    fn learn(path: &Path) -> Self {
        let mut decoder = Decoder::open_with(path, decoder_options()).expect("the fixture decodes");
        let mut seconds: BTreeMap<u32, BurnIn> = BTreeMap::new();
        let mut index = 0;
        while let Some(frame) = decoder.next_frame().expect("the fixture decodes") {
            let second = timecode(index).seconds();
            seconds
                .entry(second)
                .or_insert_with(|| BurnIn::of_frame(&frame));
            index += 1;
        }
        assert_eq!(index, FRAME_COUNT, "the fixture is 125 frames long");
        assert_eq!(seconds.len(), 5, "a five second clip shows five seconds");
        // Different seconds have to look clearly different, or reading one back
        // off a canvas would be guesswork.
        let digits: Vec<(&u32, &BurnIn)> = seconds.iter().collect();
        for (i, (second, window)) in digits.iter().enumerate() {
            for (other, other_window) in &digits[i + 1..] {
                let differing = window.differing(other_window);
                assert!(
                    differing >= window.different_digit(),
                    "seconds {second} and {other} are only {differing} pixels apart"
                );
            }
        }
        Self { seconds }
    }

    /// The timecode second a window shows.
    ///
    /// # Panics
    ///
    /// Panics when no learnt digit matches clearly enough, which means the
    /// picture is not this fixture's burn-in at all — a black canvas, or the
    /// wrong file.
    fn read_second(&self, window: &BurnIn, what: &str) -> u32 {
        let mut scored: Vec<(usize, u32)> = self
            .seconds
            .iter()
            .map(|(second, learnt)| (learnt.differing(window), *second))
            .collect();
        scored.sort_unstable();
        let (best, second) = scored[0];
        let (runner_up, _) = scored[1];
        // The nearest learnt second has to be within the tolerance and every
        // other one outside it: that, and not a fixed distance, is what makes
        // the reading unambiguous. `learn` has already checked that the
        // learnt seconds are twice the tolerance apart, so a canvas this
        // close to one of them cannot be that close to another.
        assert!(
            best <= window.same_digit() && runner_up > window.same_digit(),
            "{what}: the burnt-in timecode could not be read off the viewer: \
             best '{second}' at {best} pixels, next at {runner_up}"
        );
        second
    }
}

/// Composites one frame of the fixture the way the viewer does, on `context`.
///
/// The same decode, the same NV12 upload and the same compositor the preview
/// runs — a second instance of the production path — so the canvas it produces
/// is what the viewer's canvas has to be.
struct Reference {
    decoder: Decoder,
    index: std::sync::Arc<PtsIndex>,
    compositor: Compositor,
    context: RenderContext,
    sequence: Sequence,
}

impl Reference {
    /// Opens the fixture and a compositor for `sequence`.
    fn open(context: &RenderContext, path: &Path, sequence: &Sequence) -> Self {
        let index = std::sync::Arc::new(PtsIndex::build(path).expect("the fixture indexes"));
        let mut decoder = Decoder::open_with(path, decoder_options()).expect("the fixture decodes");
        decoder.set_index(std::sync::Arc::clone(&index));
        Self {
            decoder,
            index,
            compositor: Compositor::for_sequence(context.clone(), sequence),
            context: context.clone(),
            sequence: sequence.clone(),
        }
    }

    /// The decoded picture of the fixture's frame `frame`.
    fn frame(&mut self, frame: usize) -> VideoFrame {
        let pts = self.index.pts(frame).expect("the frame is in the index");
        self.decoder
            .seek_to(pts)
            .expect("the seek succeeds")
            .expect("a picture at the target")
    }

    /// That picture composited onto the sequence canvas, read back as RGBA.
    fn canvas(&mut self, frame: usize) -> Vec<u8> {
        let picture = self.frame(frame);
        let geometry = Nv12Geometry::new(
            picture.width(),
            picture.height(),
            picture.plane_stride(0).unwrap_or(0),
            picture.plane_stride(1).unwrap_or(0),
        )
        .expect("a usable NV12 geometry");
        let converter = Nv12Converter::new(self.context.device(), geometry);
        converter
            .submit_frame(
                self.context.device(),
                self.context.queue(),
                picture.plane_data(0).unwrap_or(&[]),
                picture.plane_data(1).unwrap_or(&[]),
            )
            .expect("the picture uploads");
        let view = converter
            .output()
            .create_view(&wgpu::TextureViewDescriptor::default());
        let source = SourceFrame::new(view, geometry.width(), geometry.height());
        let time = RationalTime::new(i64::try_from(frame).expect("a small frame"), rate());
        let mut frames = |_: &ResolvedClip<'_>| -> Option<SourceFrame> { Some(source.clone()) };
        self.compositor.render(&self.sequence, time, &mut frames);
        self.compositor.read_rgba()
    }
}

/// How far apart two canvases are: the mean absolute difference per channel.
///
/// Both go through the same conversion shader on the same device, so an
/// identical picture comes back identical; the mean is what says *how* wrong a
/// wrong one is when it fails.
fn mean_difference(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "two canvases of one size");
    let total: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    #[allow(
        clippy::cast_precision_loss,
        reason = "a mean over millions of bytes, printed on failure"
    )]
    {
        total as f64 / a.len() as f64
    }
}

/// Whether a canvas is the bare black one the viewer drew before this task.
fn is_black(rgba: &[u8]) -> bool {
    rgba.chunks_exact(4).all(|pixel| pixel[..3] == [0, 0, 0])
}

/// The editor, opened on `project`.
fn app_harness(project: &Path) -> Harness<'static, SubordinateApp> {
    let options = AppOptions {
        project: Some(project.to_path_buf()),
        ..AppOptions::default()
    };
    support::builder::<SubordinateApp>()
        .with_size(WINDOW_SIZE)
        .build_eframe(move |cc| SubordinateApp::new(cc, options).expect("the editor starts"))
}

/// Presses `key` `times` times, painting a frame for each.
///
/// `step` rather than `run`: the window asks for another frame while a
/// decoder is still catching up, and `run` treats a pending repaint request as
/// a UI that will not settle. Here it is a decoder that has not landed yet,
/// and [`settled_canvas`] is what waits for it.
fn press(harness: &mut Harness<'static, SubordinateApp>, key: Key, times: usize) {
    for _ in 0..times {
        harness.key_press_modifiers(Modifiers::NONE, key);
        harness.step();
    }
}

/// Paints until the preview has the picture the playhead is on, and returns
/// the canvas.
///
/// This is the only place the test waits, and it waits by painting: the window
/// asks for the next frame itself while a decoder is still catching up, so a
/// settled preview is one where a repaint changes nothing. A run that never
/// settles fails here rather than comparing a half-decoded picture.
fn settled_canvas(harness: &mut Harness<'static, SubordinateApp>, what: &str) -> Vec<u8> {
    for _ in 0..SETTLE_FRAMES {
        harness.step();
        if harness.state().previews().settled() {
            // The last composite had the picture every layer under the
            // playhead wanted: the viewer is showing the frame it is on.
            return harness.state().compositor().read_rgba();
        }
        // A painted frame costs the harness no wall time, and the decoders are
        // on threads of their own: without this the loop would spin through
        // its whole budget before a worker had been scheduled once. The window
        // itself never sleeps — it asks for a repaint and returns.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let stats = harness.state().previews().stats();
    let failures = harness.state().previews().failures();
    panic!("{what}: the preview never caught up with the playhead ({stats:?}, {failures:?})");
}

/// Asserts that the window's canvas is the fixture's frame `frame`.
fn assert_shows_frame(
    harness: &mut Harness<'static, SubordinateApp>,
    key: &BurnInKey,
    reference: &mut Reference,
    frame: usize,
    what: &str,
) {
    assert_eq!(
        harness.state_mut().viewer().state.playhead_frame(),
        i64::try_from(frame).expect("a small frame"),
        "{what}: the playhead is not on frame {frame}"
    );
    let canvas = settled_canvas(harness, what);
    assert!(
        !is_black(&canvas),
        "{what}: the viewer is still drawing the bare black canvas"
    );

    // The burnt-in timecode names the right second.
    let window = BurnIn::of_rgba(&canvas, 1920, 1080);
    assert_eq!(
        key.read_second(&window, what),
        timecode(frame).seconds(),
        "{what}: the burnt-in timecode must read {}",
        timecode(frame)
    );

    // And the whole picture is that frame's, against the same frame decoded
    // out of the fixture and composited through the same shader.
    let expected = reference.canvas(frame);
    let difference = mean_difference(&canvas, &expected);
    assert!(
        difference < 1.0,
        "{what}: the viewer's canvas differs from frame {frame} of the \
         fixture by {difference:.3} per channel"
    );
}

#[test]
fn scrubbing_shows_the_decoded_frame_the_playhead_is_on() {
    let Some(fixture) = fixture() else { return };
    if !support::can_render() {
        return;
    }
    let index = PtsIndex::build(&fixture).expect("the fixture indexes");
    let first = index.pts(0).expect("a first picture");
    let project = project_over(&fixture, "scrub", first);
    let key = BurnInKey::learn(&fixture);

    let mut harness = app_harness(&project);
    // The window opens the project, the engine publishes it and the preview
    // job opens the pipeline; none of that is instant, and none of it blocks.
    harness.step();
    let sequence = harness
        .state()
        .session()
        .project()
        .sequences
        .first()
        .cloned()
        .expect("the project opened with its sequence");
    let context = harness.state().render_context().clone();
    let mut reference = Reference::open(&context, &fixture, &sequence);

    // Frame zero, before anything has been scrubbed.
    assert_shows_frame(&mut harness, &key, &mut reference, 0, "at the head");

    // A scrub forward, from the keyboard, through the window's own keymap.
    // Thirty frames crosses a timecode second, so the burn-in has to have
    // changed by the end of it.
    press(&mut harness, Key::ArrowRight, 30);
    assert_shows_frame(&mut harness, &key, &mut reference, 30, "scrubbed forward");

    // And back again. A backward step is the one the TASK-133 GOP cache
    // serves: the decoder has just decoded this ground.
    press(&mut harness, Key::ArrowLeft, 12);
    assert_shows_frame(&mut harness, &key, &mut reference, 18, "scrubbed back");

    // A jump the ring cannot reach, which is a real seek to another GOP.
    harness.state_mut().viewer().state.seek_to_frame(100);
    harness.step();
    assert_shows_frame(&mut harness, &key, &mut reference, 100, "jumped");

    // Nothing here ever waited on a decoder: every picture above was taken
    // from a painted frame, and the preview counts what it decoded.
    let stats = harness.state().previews().stats();
    assert_eq!(stats.open, 1, "one pipeline for one clip: {stats:?}");
    assert_eq!(stats.failed, 0, "no clip failed to open: {stats:?}");
    assert!(
        stats.delivered >= 4,
        "every landing delivered a picture: {stats:?}"
    );
    assert_eq!(
        stats.showing, 1,
        "the ready line's picture count is the layer the viewer is showing: {stats:?}"
    );
}

#[test]
fn a_paint_never_waits_for_the_picture_it_asked_for() {
    // The §4 rule, stated as something that can fail: opening a pipeline
    // builds a PTS index over the whole file, and the window has to keep
    // painting through it. If any of that ran on the UI thread the first
    // paints would not return until the decoder was open, and `opening` would
    // never be seen from here.
    let Some(fixture) = fixture() else { return };
    if !support::can_render() {
        return;
    }
    let index = PtsIndex::build(&fixture).expect("the fixture indexes");
    let first = index.pts(0).expect("a first picture");
    let project = project_over(&fixture, "nonblocking", first);
    let mut harness = app_harness(&project);

    // Drive the window from cold — the project opening, the decoder opening,
    // then a scrub that keeps moving the playhead off whatever has just been
    // decoded — and count the paints that came back *before* the picture they
    // asked for existed. A window that waited for its decoder could not
    // produce one: every paint would return with the frame already in hand.
    // Those unfinished paints are the whole of the rule, and they are what
    // lets the viewer stay responsive under a scrub it cannot keep up with.
    let mut waited_on_nothing = 0;
    for _ in 0..SETTLE_FRAMES {
        harness.step();
        if harness.state().previews().settled() {
            // Landed: move on, so the next paint has a decode outstanding.
            harness.state_mut().viewer().state.step_frames(7);
        } else {
            waited_on_nothing += 1;
        }
        if harness.state().previews().stats().delivered >= 4 {
            break;
        }
        // Outside the paint: the decoders are on threads of their own and a
        // painted frame costs this harness no wall time, so without a pause
        // the loop would outrun every worker.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let stats = harness.state().previews().stats();
    assert!(
        stats.delivered >= 4,
        "the preview never put pictures on screen: {stats:?}"
    );
    assert!(
        waited_on_nothing > 0,
        "every paint came back with its picture already decoded, so no paint \
         ever handed the decode away: {stats:?}"
    );
    assert_eq!(stats.failed, 0, "no clip failed to open: {stats:?}");
}

#[test]
fn playing_follows_the_playhead_out_of_the_decode_ahead_ring() {
    // Criterion 2's picture half. The transport is run from the keyboard and
    // the viewer is asked, at each landing, whether it is showing the frame
    // the playhead reached. What makes that possible without a seek a frame is
    // the decode-ahead ring, which the preview reads from while the transport
    // is running; the drop counters are read out at the end.
    let Some(fixture) = fixture() else { return };
    if !support::can_render() {
        return;
    }
    let index = PtsIndex::build(&fixture).expect("the fixture indexes");
    let first = index.pts(0).expect("a first picture");
    let project = project_over(&fixture, "play", first);
    let key = BurnInKey::learn(&fixture);

    let mut harness = app_harness(&project);
    harness.step();
    let sequence = harness
        .state()
        .session()
        .project()
        .sequences
        .first()
        .cloned()
        .expect("the project opened with its sequence");
    let context = harness.state().render_context().clone();
    let mut reference = Reference::open(&context, &fixture, &sequence);
    // Settle on the head first, so what follows is playback rather than the
    // first open.
    let _ = settled_canvas(&mut harness, "before playing");

    // Space is play/pause in the shipped keymap. The clock is the monotonic
    // fallback here: this harness opens no audio device, which is exactly the
    // case `run_transport` falls back for.
    harness.key_press_modifiers(Modifiers::NONE, Key::Space);
    let mut landings = 0;
    let mut last = 0_i64;
    for _ in 0..SETTLE_FRAMES {
        harness.step();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let frame = harness.state_mut().viewer().state.playhead_frame();
        // Whether the picture had caught up at this instant is not the
        // question here — during playback it is a frame behind as often as
        // not, which is what the ring is for. The question is whether the
        // transport moved and the preview followed, and the counters below
        // answer the second half.
        if frame > last {
            last = frame;
            landings += 1;
            if landings >= 3 {
                break;
            }
        }
    }
    // Which master the transport was following is the first thing to know
    // when this fails: a window that opened an output device follows its
    // clock, and one that did not follows the monotonic fallback. These
    // harnesses ask for no device (`AppOptions::open_audio_output`), so this
    // should say "no".
    let audio_open = harness.state_mut().audio().is_open();
    let master = harness
        .state_mut()
        .audio()
        .clock()
        .and_then(|clock| clock.position());
    assert!(
        landings >= 3,
        "playback never moved the playhead past frame {last} \
         (audio output open: {audio_open}, master: {master:?})"
    );

    // Pause, and check the picture on screen really is the frame it stopped
    // on — the same whole-picture comparison the scrub half makes.
    harness.key_press_modifiers(Modifiers::NONE, Key::Space);
    harness.step();
    let frame = usize::try_from(harness.state_mut().viewer().state.playhead_frame())
        .expect("a small frame");
    assert_shows_frame(&mut harness, &key, &mut reference, frame, "after playing");

    let stats = harness.state().previews().stats();
    assert_eq!(stats.failed, 0, "no clip failed to open: {stats:?}");
    assert!(
        stats.delivered >= 3,
        "playback put pictures on screen: {stats:?}"
    );
    // The counters exist and are read: a picture decoded that the playhead had
    // already run past, or thrown out of a ring by a seek, is counted rather
    // than lost silently. A run this short on this machine need not drop any.
    println!("preview after playback: {stats:?}");
}

#[test]
fn the_head_of_the_timeline_is_not_a_black_canvas() {
    // The narrowest statement of the bug this task is about, kept separate so
    // a failure says whether the viewer draws *anything* or draws the *wrong*
    // thing.
    let Some(fixture) = fixture() else { return };
    if !support::can_render() {
        return;
    }
    let index = PtsIndex::build(&fixture).expect("the fixture indexes");
    let first = index.pts(0).expect("a first picture");
    let project = project_over(&fixture, "black", first);
    let mut harness = app_harness(&project);
    harness.step();
    let canvas = settled_canvas(&mut harness, "at the head");
    assert!(
        !is_black(&canvas),
        "the viewer composites the decoded clip, not the bare canvas"
    );
}

#[test]
fn a_clip_whose_media_is_gone_does_not_hold_the_window_back() {
    // The window holds its ready line until the preview has the picture the
    // playhead is on, which is what makes an unattended screenshot worth
    // trusting. A clip whose file is not on this machine will never have a
    // picture, so it must not be something the window waits for — otherwise a
    // project with one offline clip would never report ready at all.
    let Some(fixture) = fixture() else { return };
    if !support::can_render() {
        return;
    }
    let index = PtsIndex::build(&fixture).expect("the fixture indexes");
    let first = index.pts(0).expect("a first picture");
    let project = project_over(&fixture, "offline", first);
    std::fs::remove_file(project.with_file_name("bars.mp4")).expect("the media goes away");

    let mut harness = app_harness(&project);
    for _ in 0..SETTLE_FRAMES {
        harness.step();
        if harness.state().previews().settled() && harness.state().windows_are_up() {
            let stats = harness.state().previews().stats();
            assert_eq!(stats.showing, 0, "there is no picture to show: {stats:?}");
            assert_eq!(stats.open, 0, "no pipeline was opened: {stats:?}");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!(
        "the window never came up over an offline clip: {:?}",
        harness.state().previews().stats()
    );
}
