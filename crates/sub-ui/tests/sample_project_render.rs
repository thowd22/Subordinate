//! The CI render test over the sample project (TASK-109).
//!
//! `examples/sample-project/demo.sub` is the ready-made project a new user
//! opens; this is the test that proves it is more than a document. It takes
//! the committed project, resolves the layers under a playhead exactly as the
//! viewer does, decodes those frames out of the real CC0 files
//! `scripts/get-sample-media.sh` fetched, uploads them as NV12 and composites
//! them through `sub-render`. Five properties are checked against the result:
//!
//! - the media on disk is **the media the project names**: every item's
//!   recorded content hash matches the file the fetch script downloaded, which
//!   is also what "opens without relinking" means once the path has resolved;
//! - a plain cut **draws a picture**: the composite of the first clip is not
//!   the empty black canvas;
//! - the overlay track **stacks**: with V2 under the playhead two layers are
//!   drawn and the canvas differs from the same frame with V2 suppressed;
//! - the crossfade **blends**: at the middle of the dissolve V1 yields both of
//!   its clips, the incoming one at half weight, and the canvas differs from
//!   the same frame with the incoming clip suppressed;
//! - the first-party colour plugin **runs on it**: the effect the project
//!   itself applies to the wide shot — `com.subordinate.color`, a stop up with
//!   a warm tint — is resolved against `plugins/color`'s own declaration,
//!   bound with the values stored in `demo.sub` and run, and the canvas comes
//!   back brighter with no effect failures.
//!
//! The declaration — the parameter table and the WGSL — belongs to the plugin
//! and is compiled out of its source tree here, exactly as the host lifts it
//! out of the installed component; what the *project* stores is the reference
//! and the bound values, which is what this test reads.
//!
//! The test skips itself, reporting why, when the sample media has not been
//! fetched or the machine enumerates no wgpu adapter, so `cargo test` works on
//! a fresh checkout; CI runs `scripts/get-sample-media.sh` first and has a
//! software adapter, so there it really executes. CI also sets
//! [`REQUIRE_MEDIA`], which turns the media skip into a failure: "the sample
//! project opens without relinking on this OS" is only proven by a run that
//! really resolved every media path, so a silent skip there would prove
//! nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use eframe::wgpu;
use sub_media::{Decoder, DecoderOptions, FrameFormat, StreamSelection};
use sub_model::content::ContentHash;
use sub_model::effect::{ClipEffect, EffectValue};
use sub_model::params::Fixed6;
use sub_model::{ClipId, Project, Sequence, json};
use sub_render::{
    Compositor, EffectDesc, EffectInstance, EffectParam, EffectSource, Nv12Converter, Nv12Geometry,
    ParamKind, ParamValue, RenderContext, RenderError, ResolvedClip, SourceFrame,
    resolve_layers_at,
};
use sub_time::{Rational, RationalTime};

/// The plugin's parameter table and its CPU reference, compiled straight out
/// of the plugin's source tree so there is only one of each.
#[path = "../../../plugins/color/src/grade.rs"]
mod grade;

/// The plugin's shader: the file the component ships.
const SHADER: &str = include_str!("../../../plugins/color/src/effect.wgsl");

/// The fragment entry point declared in that file.
const ENTRY: &str = "fs_main";

/// The sequence the render test drives.
const SEQUENCE: &str = "Main cut";

/// The clip the project drops the grade on.
const GRADED_CLIP: &str = "Porters, wide";

/// The plugin the project's effect names, which is the one compiled in above.
const COLOR_PLUGIN: &str = "com.subordinate.color";

/// The clip on the overlay track.
const OVERLAY_CLIP: &str = "Soneros, corner";

/// The clip the dissolve brings in.
const INCOMING_CLIP: &str = "Pigeon, fight";

/// How many codes two composites must differ by, averaged over the canvas,
/// before the difference counts as a layer that really contributed. A canvas
/// that changed by less than this is noise; the pictures here are a 1921
/// black-and-white film and a colour close-up, so a genuine contribution is
/// worth tens of codes.
const DIFFERENCE: f64 = 2.0;

/// Held for the length of a test: only one test may talk to the driver.
static DRIVER: Mutex<()> = Mutex::new(());
/// The one context, built under `DRIVER`. `None` means no usable adapter.
static CONTEXT: OnceLock<Option<RenderContext>> = OnceLock::new();

/// Exclusive use of the shared context, or `None` when this machine has no
/// usable adapter. The guard must outlive every use of the context.
fn context_or_skip() -> Option<(MutexGuard<'static, ()>, RenderContext)> {
    let guard = DRIVER.lock().unwrap_or_else(PoisonError::into_inner);
    let context = CONTEXT.get_or_init(|| match RenderContext::headless() {
        Ok(context) => Some(context),
        Err(RenderError::NoAdapter { backends }) => {
            eprintln!("skipping: no wgpu adapter for backends [{backends}]");
            None
        }
        Err(error) => panic!("[{}] {error}", error.code()),
    });
    context.clone().map(|context| (guard, context))
}

/// Environment variable that forbids the media skip: set where a skip would
/// hide the very thing the run is meant to prove.
const REQUIRE_MEDIA: &str = "SUB_REQUIRE_SAMPLE_MEDIA";

/// The directory `demo.sub` lives in, which is what every media path in it is
/// relative to.
fn project_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sample-project")
}

/// The sample project, or `None` when its media has not been fetched.
///
/// The project itself is committed, so a missing file here is a broken
/// checkout rather than a skip; the *media* is the part a fresh checkout does
/// not have.
fn project_or_skip() -> Option<Project> {
    let path = project_dir().join("demo.sub");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("could not read {}: {err}", path.display()));
    let project = json::from_json(&text).expect("the sample project loads");
    for item in &project.media {
        let resolved = item.path.resolve(&project_dir());
        if !resolved.is_file() {
            assert!(
                std::env::var_os(REQUIRE_MEDIA).is_none(),
                "{REQUIRE_MEDIA} is set, so the sample project must open with every media \
                 path resolved, but {} is not a file; run scripts/get-sample-media.sh",
                resolved.display()
            );
            eprintln!(
                "skipping: no sample media at {}; run scripts/get-sample-media.sh",
                resolved.display()
            );
            return None;
        }
    }
    Some(project)
}

/// The sequence the test drives, by name.
fn sequence(project: &Project) -> &Sequence {
    project
        .sequences
        .iter()
        .find(|sequence| sequence.name == SEQUENCE)
        .expect("the sample project has a main sequence")
}

/// A time on that sequence, in frames of its own timebase.
fn frame(project: &Project, frames: i64) -> RationalTime {
    RationalTime::new(frames, sequence(project).settings.frame_rate)
}

/// The decoded pictures for one playhead position, uploaded and converted.
///
/// The converters own the textures the views point at, so they are kept
/// alongside them for as long as the frames are used.
struct Pictures {
    frames: BTreeMap<ClipId, SourceFrame>,
    _converters: Vec<Nv12Converter>,
}

impl Pictures {
    /// A [`sub_render::FrameSource`] over what was decoded.
    fn source(&self) -> impl FnMut(&ResolvedClip<'_>) -> Option<SourceFrame> {
        |clip: &ResolvedClip<'_>| self.frames.get(&clip.clip_id()).cloned()
    }
}

/// Decode every layer under `time`, skipping clips named in `without`.
///
/// One decoder per layer, opened and seeked: the sample project is small and
/// this is a test, so nothing is cached between calls. `without` is how a
/// composite is rendered *as if* a layer were not there, which is what the
/// stacking and crossfade assertions compare against.
fn pictures(
    context: &RenderContext,
    project: &Project,
    time: RationalTime,
    without: &[&str],
) -> Pictures {
    let dir = project_dir();
    let mut frames = BTreeMap::new();
    let mut converters = Vec::new();
    for resolved in resolve_layers_at(sequence(project), time) {
        if without.contains(&resolved.clip.name.as_str()) {
            continue;
        }
        let item = project
            .media
            .iter()
            .find(|item| item.id == resolved.clip.media)
            .expect("every clip names a media item in the project");
        let path = item.path.resolve(&dir);
        let options = DecoderOptions {
            streams: StreamSelection::Video,
            format: FrameFormat::Nv12,
            frame_timeout: Duration::from_secs(30),
            ..DecoderOptions::default()
        };
        let mut decoder = Decoder::open_with(&path, options)
            .unwrap_or_else(|err| panic!("[{}] {} {err}", err.code, path.display()));
        let picture = decoder
            .seek_to(resolved.source_time)
            .unwrap_or_else(|err| panic!("[{}] seeking {} {err}", err.code, path.display()))
            .unwrap_or_else(|| panic!("{} has no frame at its source time", path.display()));

        let (width, height) = (picture.width(), picture.height());
        let geometry = Nv12Geometry::new(
            width,
            height,
            picture.plane_stride(0).expect("a luma stride"),
            picture.plane_stride(1).expect("a chroma stride"),
        )
        .expect("the decoder's own geometry is a valid one");
        let converter = Nv12Converter::new(context.device(), geometry);
        converter
            .submit_frame(
                context.device(),
                context.queue(),
                picture.plane_data(0).expect("a luma plane"),
                picture.plane_data(1).expect("a chroma plane"),
            )
            .expect("the decoded planes upload");
        let view = converter
            .output()
            .create_view(&wgpu::TextureViewDescriptor::default());
        frames.insert(
            resolved.clip_id(),
            SourceFrame::new(view, geometry.width(), geometry.height()),
        );
        converters.push(converter);
    }
    Pictures {
        frames,
        _converters: converters,
    }
}

/// Render `time` with the layers named in `without` left out, and read the
/// canvas back as tightly packed RGBA.
fn render(
    context: &RenderContext,
    project: &Project,
    time: RationalTime,
    without: &[&str],
    effects: &mut dyn EffectSource,
) -> (Vec<u8>, sub_render::FrameSummary) {
    let pictures = pictures(context, project, time, without);
    let mut compositor = Compositor::for_sequence(context.clone(), sequence(project));
    let mut source = pictures.source();
    let summary = compositor.render_with_effects(sequence(project), time, &mut source, effects);
    (compositor.read_rgba(), summary)
}

/// The mean of every colour channel on the canvas, in sRGB codes.
#[expect(
    clippy::cast_precision_loss,
    reason = "a canvas holds far fewer samples, and a far smaller sum, than f64 counts exactly"
)]
fn mean(pixels: &[u8]) -> f64 {
    let sum: u64 = pixels
        .chunks_exact(4)
        .flat_map(|pixel| pixel[..3].iter().map(|&code| u64::from(code)))
        .sum();
    let samples = (pixels.len() / 4 * 3) as f64;
    sum as f64 / samples
}

/// The mean absolute difference between two canvases, in sRGB codes.
#[expect(
    clippy::cast_precision_loss,
    reason = "a canvas holds far fewer samples, and a far smaller sum, than f64 counts exactly"
)]
fn difference(left: &[u8], right: &[u8]) -> f64 {
    assert_eq!(left.len(), right.len(), "two canvases of the same size");
    let sum: u64 = left
        .chunks_exact(4)
        .zip(right.chunks_exact(4))
        .flat_map(|(a, b)| {
            a[..3]
                .iter()
                .zip(&b[..3])
                .map(|(&x, &y)| u64::from(x.abs_diff(y)))
        })
        .sum();
    let samples = (left.len() / 4 * 3) as f64;
    sum as f64 / samples
}

/// The plugin's declaration as the host builds it, lifted out of the plugin's
/// own parameter table.
fn color_effect() -> Arc<EffectDesc> {
    let params = grade::PARAMS
        .iter()
        .map(|spec| {
            let kind = match spec.kind {
                grade::SpecKind::Float { min, max, default } => ParamKind::Float {
                    min,
                    max,
                    default,
                    step: None,
                },
                grade::SpecKind::Color { default } => ParamKind::Color { default },
            };
            EffectParam::new(spec.id, spec.label, kind).with_doc(spec.doc)
        })
        .collect();
    Arc::new(
        EffectDesc::new(params, SHADER, ENTRY).expect("the plugin's declaration is a valid one"),
    )
}

/// Bind one stored [`ClipEffect`] against the declaration of the plugin it
/// names.
///
/// This is the host's job in miniature: the project carries a plugin id and a
/// handful of values, the installed plugin carries the parameters and the
/// shader, and binding the two is what the compositor is handed. A value for a
/// parameter the plugin no longer declares is never packed, and a parameter
/// the project says nothing about keeps its declared default.
fn bind(desc: &Arc<EffectDesc>, effect: &ClipEffect) -> EffectInstance {
    let mut instance = EffectInstance::new(Arc::clone(desc));
    for (id, value) in &effect.params {
        instance.set(id.clone(), param_value(*value));
    }
    instance
}

/// One stored value as the compositor's own parameter value.
fn param_value(value: EffectValue) -> ParamValue {
    match value {
        EffectValue::Float(value) => ParamValue::Float(value.as_f32()),
        EffectValue::Int(value) => ParamValue::Int(value),
        EffectValue::Bool(value) => ParamValue::Bool(value),
        EffectValue::Color(channels) => ParamValue::Color(channels.map(Fixed6::as_f32)),
        EffectValue::Choice(index) => ParamValue::Choice(index),
    }
}

/// An [`EffectSource`] that runs exactly what the project applies: every
/// enabled effect on the clip, in stored order, bound against the plugin that
/// declares it.
///
/// `plugins/color` is the only plugin the sample project names, and it is
/// compiled in above; an effect naming anything else is a project this test
/// cannot honour, so it fails loudly rather than quietly rendering a clip
/// ungraded.
fn project_effects(desc: Arc<EffectDesc>) -> impl FnMut(&ResolvedClip<'_>) -> Vec<EffectInstance> {
    move |clip: &ResolvedClip<'_>| {
        clip.clip
            .effects
            .iter()
            .filter(|effect| effect.enabled)
            .map(|effect| {
                assert_eq!(
                    effect.plugin, COLOR_PLUGIN,
                    "the sample project applies an effect from a plugin this test does not have"
                );
                bind(&desc, effect)
            })
            .collect()
    }
}

#[test]
fn the_sample_media_is_the_media_the_project_names() {
    let Some(project) = project_or_skip() else {
        return;
    };
    for item in &project.media {
        let path = item.path.resolve(&project_dir());
        let hash = ContentHash::of_file(&path).expect("the media file hashes");
        assert_eq!(
            item.hash,
            Some(hash),
            "{} is not the file the sample project was built against; \
             re-run scripts/get-sample-media.sh",
            item.path.as_str()
        );
        let info = item
            .info
            .as_ref()
            .expect("every sample media item is probed");
        assert!(!info.video.is_empty(), "{} has video", item.path.as_str());
    }
}

#[test]
fn the_first_clip_composites_a_real_picture() {
    let Some(project) = project_or_skip() else {
        return;
    };
    let Some((_guard, context)) = context_or_skip() else {
        return;
    };
    let time = frame(&project, 25);
    let (pixels, summary) = render(&context, &project, time, &[], &mut sub_render::NoEffects);

    assert_eq!(summary.drawn(), 1, "one clip is under the playhead at 1 s");
    let settings = sequence(&project).settings;
    assert_eq!(
        pixels.len(),
        (settings.resolution.width() * settings.resolution.height() * 4) as usize
    );
    assert!(
        mean(&pixels) > 8.0,
        "the composite is as good as black: the decoded frame never reached the canvas"
    );
}

#[test]
fn the_overlay_track_stacks_over_the_programme_track() {
    let Some(project) = project_or_skip() else {
        return;
    };
    let Some((_guard, context)) = context_or_skip() else {
        return;
    };
    // Frame 112 is inside both the first clip and the overlay that starts at
    // frame 100.
    let time = frame(&project, 112);
    let (stacked, summary) = render(&context, &project, time, &[], &mut sub_render::NoEffects);
    assert_eq!(summary.drawn(), 2, "the programme clip and the overlay");

    let (alone, _) = render(
        &context,
        &project,
        time,
        &[OVERLAY_CLIP],
        &mut sub_render::NoEffects,
    );
    assert!(
        difference(&stacked, &alone) > DIFFERENCE,
        "the overlay changed nothing on the canvas"
    );
}

#[test]
fn the_crossfade_blends_both_clips_at_its_midpoint() {
    let Some(project) = project_or_skip() else {
        return;
    };
    let Some((_guard, context)) = context_or_skip() else {
        return;
    };
    // The cut is at frame 150 and the dissolve reaches 12 frames either side
    // of it, so the cut itself is the half-way point of the blend.
    let time = frame(&project, 150);
    // Three layers: the two halves of the dissolve on V1, and the overlay
    // clip on V2, which runs over the second half of the sequence and so is
    // under this playhead too.
    let layers: Vec<_> = resolve_layers_at(sequence(&project), time).collect();
    assert_eq!(
        layers
            .iter()
            .map(|layer| layer.clip.name.as_str())
            .collect::<Vec<_>>(),
        vec![GRADED_CLIP, INCOMING_CLIP, OVERLAY_CLIP],
        "the dissolve resolves to both of its clips, bottom track first"
    );
    assert!(
        layers[0].blend.as_f32() > 0.999,
        "the outgoing clip plays on under the blend"
    );
    assert!(
        (layers[1].blend.as_f32() - 0.5).abs() < 0.001,
        "the incoming clip is at half weight at the middle of the blend, was {}",
        layers[1].blend.as_f32()
    );

    let (blended, summary) = render(&context, &project, time, &[], &mut sub_render::NoEffects);
    assert_eq!(summary.drawn(), 3);
    let (outgoing_only, _) = render(
        &context,
        &project,
        time,
        &[INCOMING_CLIP],
        &mut sub_render::NoEffects,
    );
    assert!(
        difference(&blended, &outgoing_only) > DIFFERENCE,
        "the incoming clip contributed nothing to the dissolve"
    );
}

#[test]
fn the_grade_the_project_applies_runs_over_the_sample_project() {
    let Some(project) = project_or_skip() else {
        return;
    };
    let Some((_guard, context)) = context_or_skip() else {
        return;
    };

    // The effect is the project's, not this test's: the wide shot carries it
    // in demo.sub, and this is where it is read back.
    let graded_clip = project.sequences[0].tracks[0].items[0]
        .as_clip()
        .expect("V1 opens on the wide shot");
    assert_eq!(graded_clip.name, GRADED_CLIP);
    let stored = graded_clip
        .effects
        .first()
        .expect("the sample project applies an effect to the wide shot");
    assert_eq!(stored.plugin, COLOR_PLUGIN);
    assert_eq!(
        stored.param("exposure"),
        Some(EffectValue::Float(Fixed6::ONE)),
        "the stored grade lifts the clip a stop"
    );

    let time = frame(&project, 25);
    let (plain, _) = render(&context, &project, time, &[], &mut sub_render::NoEffects);

    let mut effects = project_effects(color_effect());
    let (graded, summary) = render(&context, &project, time, &[], &mut effects);

    assert!(
        !summary.has_effect_failures(),
        "the plugin's shader did not run: {:?}",
        summary.effect_failures
    );
    assert_eq!(summary.effects_applied(), 1, "one graded layer");
    assert!(
        mean(&graded) > mean(&plain) + 8.0,
        "a stop of exposure left the canvas no brighter: {} against {}",
        mean(&graded),
        mean(&plain)
    );
}

#[test]
fn the_sample_sequence_runs_at_its_own_timebase() {
    let Some(project) = project_or_skip() else {
        return;
    };
    // Not a render: the guard that the times this test hard-codes are frames
    // of the sequence the sample project actually ships.
    let settings = sequence(&project).settings;
    assert_eq!(settings.frame_rate, Rational::FPS_25);
    assert_eq!(
        (settings.resolution.width(), settings.resolution.height()),
        (1280, 720)
    );
}
