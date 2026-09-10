//! The phase 2 exit criterion, automated: a whole edit assembled through the
//! Command API and nothing else (docs/PLAN.md §4).
//!
//! One engine, one Command API server on its own local socket and one client
//! that talks to it over the wire. Every mutation in this file is a JSON-RPC
//! call — `media.import`, `sequence.create`, `track.add`, `clip.add`,
//! `clip.split`, `clip.trim_in`, `clip.trim_out`, `transition.add` — so what
//! the test proves is the path an agent uses, not a shortcut through
//! [`sub_edit::History`]. The result is a 20-clip, 3-track sequence with
//! splits, trims and three crossfades.
//!
//! Three properties are then checked on that edit:
//!
//! 1. It is what was asked for: 20 clips over 3 video tracks, three
//!    transitions, and a crossfade that spans frame 100.
//! 2. Undoing every step empties the project, and redoing every step gives
//!    back the *same* project file, character for character: `project.get`
//!    hands back the model in the shape a `.sub` file stores it, with its
//!    keys sorted, so a command whose inverse loses a field fails here.
//! 3. Frame 100 of the redone project renders to a picture that is not black.
//!    The playhead sits in the middle of a crossfade there, so the composite
//!    is two layers blended, which is the phase 2 picture path end to end.
//!
//! The render half needs a wgpu adapter. Where the machine has none — a
//! container with no ICD — that test reports it and passes rather than
//! failing the build on an environment problem, exactly as the compositor's
//! own tests do.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use sub_command::transport::{Client, Server};
use sub_command::{Dispatcher, Endpoint};
use sub_edit::Engine;
use sub_edit::clip::{AddClip, SplitClip, TrimClipIn, TrimClipOut};
use sub_edit::commands::{AddTrack, AddTransition, CreateSequence, ImportMedia};
use sub_model::{
    Clip, ClipId, ColorTags, MediaId, MediaItem, MediaPath, Project, Resolution, Sequence,
    SequenceId, SequenceSettings, TrackId, TrackItem, TrackKind,
};
use sub_render::{Compositor, RenderContext, RenderError, ResolvedClip, SourceFrame};
use sub_time::{Rational, RationalTime, TimeRange};

/// The sequence timebase, and the rate every time in this file is counted at.
const RATE: Rational = Rational::FPS_24;

/// The frame the exit criterion renders.
const RENDER_FRAME: i64 = 100;

/// The canvas: 16:9, and small enough that a software adapter reads it back
/// in milliseconds. The sources are 16x9, so the picture fills it exactly and
/// no letterbox black creeps into the probe.
const CANVAS: (u32, u32) = (64, 36);

/// The source size every [`SourceFrame`] in this file declares.
const SOURCE: (u32, u32) = (16, 9);

/// How many clips the assembled sequence holds.
const CLIPS: usize = 20;

/// An instant in the sequence timebase.
fn at(frame: i64) -> RationalTime {
    RationalTime::new(frame, RATE)
}

/// A span in the sequence timebase.
fn span(start: i64, duration: i64) -> TimeRange {
    TimeRange::new(at(start), at(duration)).expect("a positive duration is a range")
}

/// A directory nothing else in this test binary uses.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the temp directory is writable");
    dir
}

/// A client of a live Command API server, and the server and engine behind it.
struct Editor {
    client: Client,
    server: Server,
    engine: Engine,
}

impl Editor {
    /// An empty project served over its own private endpoint.
    fn open(name: &str) -> Self {
        let engine = Engine::spawn(Project::new("Doc cut")).expect("the engine thread starts");
        let endpoint = Endpoint::in_directory(temp_dir(name), "test").expect("a usable endpoint");
        let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
        let server = Server::bind(endpoint, dispatcher).expect("the endpoint is free");
        let client = Client::connect(server.endpoint()).expect("the server is listening");
        Self {
            client,
            server,
            engine,
        }
    }

    /// Applies one command by its own kind and parameters.
    fn apply<C: sub_edit::Command>(&mut self, command: &C) -> Value {
        let params = serde_json::to_value(command).expect("a command serialises");
        self.client
            .invoke(C::KIND, Some(params))
            .unwrap_or_else(|error| panic!("[{}] {}: {error}", error.code, C::KIND))
    }

    /// Calls a method that takes no parameters.
    fn query(&mut self, method: &str) -> Value {
        self.client
            .invoke(method, None)
            .unwrap_or_else(|error| panic!("[{}] {method}: {error}", error.code))
    }

    /// The project file `project.get` hands over: the whole model wrapped in
    /// its schema version, exactly as a `.sub` file stores it.
    fn project_json(&mut self) -> String {
        let result = self.query("project.get");
        serde_json::to_string_pretty(&result["project"]).expect("the reply is JSON")
    }

    /// The same project, as the model.
    fn project(&mut self) -> Project {
        let text = self.project_json();
        sub_model::json::from_json(&text).expect("the project round-trips")
    }

    /// Shuts the client, the server and the engine down in that order.
    fn close(self) {
        drop(self.client);
        self.server.shutdown().expect("the server stops");
        self.engine.shutdown().expect("the engine stops");
    }
}

/// The identifiers the assembly hands back, for the assertions that follow.
struct Assembled {
    sequence: SequenceId,
    /// The clip the crossfade at frame 100 fades out of.
    outgoing_at_100: ClipId,
    /// The clip the crossfade at frame 100 fades into.
    incoming_at_100: ClipId,
    /// How many commands the assembly applied.
    commands: u64,
}

/// Imports three sources, builds a three-track sequence and assembles the
/// edit: 15 clips placed, 5 of them split in two, two trims and three
/// crossfades.
///
/// Every step is a Command API call, so the revision after the last one is
/// the number of commands applied.
fn assemble(editor: &mut Editor) -> Assembled {
    // Three sources. Nothing is probed, so the model knows only what a clip
    // takes from each: enough for the head handles a crossfade reaches into.
    let media: Vec<MediaId> = ["a.mp4", "b.mp4", "c.mp4"]
        .iter()
        .map(|name| {
            let item = MediaItem::new(MediaPath::new(name).expect("a relative path"));
            let id = item.id;
            editor.apply(&ImportMedia::new(item));
            id
        })
        .collect();

    let settings = SequenceSettings::new(
        Resolution::new(CANVAS.0, CANVAS.1).expect("the canvas is non-zero"),
        RATE,
        48_000,
        ColorTags::REC709,
    )
    .expect("48 kHz is a valid sample rate");
    editor.apply(&CreateSequence::new("Main", settings));
    let sequence = editor.project().sequences[0].id;

    for name in ["V1", "V2", "V3"] {
        editor.apply(&AddTrack::new(sequence, name, TrackKind::Video));
    }
    let tracks: Vec<TrackId> = editor.project().sequences[0]
        .tracks
        .iter()
        .map(|track| track.id)
        .collect();

    // Butt-joined runs, one per track. Every clip is cut from the middle of
    // its source, so each has a head handle for a crossfade to reach into.
    // Only V1 covers frame 100; the tracks above it start later, so the
    // crossfade there is what the render sees.
    let mut placed: Vec<Vec<ClipId>> = Vec::new();
    for (track_index, (&track, (name, first_frame, count))) in tracks
        .iter()
        .zip([("V1", 0_i64, 6_usize), ("V2", 240, 5), ("V3", 420, 4)])
        .enumerate()
    {
        let mut ids = Vec::with_capacity(count);
        for index in 0..count {
            let step = i64::try_from(index).expect("a small index");
            let source = span(
                120 + 45 * step + 15 * i64::try_from(track_index).unwrap(),
                30,
            );
            let clip = Clip::new(format!("{name} shot {index}"), media[index % 3], source);
            ids.push(clip.id);
            editor.apply(&AddClip {
                sequence,
                track,
                start: at(first_frame + 30 * step),
                clip,
            });
        }
        placed.push(ids);
    }

    // Five splits, so the sequence holds 20 clips. The first cuts V1 exactly
    // at frame 100, which is the cut the render's crossfade sits on.
    let splits = [
        (0_usize, 3_usize, RENDER_FRAME),
        (0, 1, 45),
        (1, 0, 255),
        (1, 2, 315),
        (2, 0, 435),
    ];
    let mut tails = Vec::with_capacity(splits.len());
    for (track_index, clip_index, frame) in splits {
        let tail = ClipId::new();
        tails.push(tail);
        editor.apply(&SplitClip {
            sequence,
            track: tracks[track_index],
            clip: placed[track_index][clip_index],
            at: at(frame),
            tail_id: Some(tail),
        });
    }

    // Two trims, both far from the rendered frame.
    editor.apply(&TrimClipOut {
        sequence,
        track: tracks[2],
        clip: placed[2][3],
        delta: at(-6),
    });
    editor.apply(&TrimClipIn {
        sequence,
        track: tracks[1],
        clip: placed[1][4],
        delta: at(6),
    });

    // Three crossfades. The first straddles frame 100: six frames each side
    // of the cut, which both halves have the handle for.
    for (track_index, clip, duration) in [
        (0_usize, tails[0], 12_i64),
        (0, placed[0][4], 8),
        (1, placed[1][1], 10),
    ] {
        editor.apply(&AddTransition::new(
            sequence,
            tracks[track_index],
            clip,
            at(duration),
        ));
    }

    Assembled {
        sequence,
        outgoing_at_100: placed[0][3],
        incoming_at_100: tails[0],
        commands: 3 + 1 + 3 + 15 + 5 + 2 + 3,
    }
}

/// Every clip on every track of a sequence.
fn clips(sequence: &Sequence) -> Vec<&Clip> {
    sequence
        .tracks
        .iter()
        .flat_map(|track| track.items.iter().filter_map(TrackItem::as_clip))
        .collect()
}

/// How many transitions a sequence holds.
fn transitions(sequence: &Sequence) -> usize {
    sequence
        .tracks
        .iter()
        .flat_map(|track| track.items.iter())
        .filter(|item| item.as_transition().is_some())
        .count()
}

#[test]
fn a_twenty_clip_edit_is_assembled_through_the_command_api() {
    let mut editor = Editor::open("assemble");
    let assembled = assemble(&mut editor);

    assert_eq!(
        editor.query("project.revision")["revision"],
        assembled.commands,
        "every step is one command and one revision",
    );

    let project = editor.project();
    let sequence = &project.sequences[0];
    assert_eq!(sequence.id, assembled.sequence);
    assert_eq!(sequence.tracks.len(), 3, "three tracks");
    assert_eq!(clips(sequence).len(), CLIPS, "twenty clips");
    assert_eq!(transitions(sequence), 3, "three crossfades");

    // The crossfade at frame 100 blends two layers there: the outgoing half
    // of the split and the incoming half over it.
    let layers: Vec<_> = sub_render::resolve_layers_at(sequence, at(RENDER_FRAME)).collect();
    assert_eq!(
        layers.len(),
        2,
        "frame {RENDER_FRAME} sits inside a crossfade",
    );
    assert_eq!(layers[0].clip_id(), assembled.outgoing_at_100);
    assert_eq!(layers[1].clip_id(), assembled.incoming_at_100);
    assert!(
        layers[1].blend.as_f32() > 0.0 && layers[1].blend.as_f32() < 1.0,
        "the incoming clip is part way through its ramp: {:?}",
        layers[1].blend,
    );

    editor.close();
}

#[test]
fn undoing_and_redoing_the_whole_edit_gives_back_the_same_project() {
    let mut editor = Editor::open("undo-redo");
    let empty = editor.project_json();
    let assembled = assemble(&mut editor);
    let assembled_json = editor.project_json();

    let mut undone = 0_u64;
    while editor.query("history.get")["can_undo"] == Value::Bool(true) {
        editor.query("edit.undo");
        undone += 1;
        assert!(undone <= assembled.commands, "undo never runs out of steps");
    }
    assert_eq!(undone, assembled.commands, "every command undoes");
    assert_eq!(
        editor.project_json(),
        empty,
        "undoing everything gives back the project the session opened with",
    );

    let mut redone = 0_u64;
    while editor.query("history.get")["can_redo"] == Value::Bool(true) {
        editor.query("edit.redo");
        redone += 1;
        assert!(redone <= assembled.commands, "redo never runs out of steps");
    }
    assert_eq!(redone, assembled.commands, "every command redoes");
    assert_eq!(
        editor.project_json(),
        assembled_json,
        "undo all then redo all yields identical project JSON",
    );

    editor.close();
}

/// A one-texel source of one colour, described to the compositor as a
/// [`SOURCE`]-sized picture so it fits the canvas exactly.
fn solid_source(context: &RenderContext, colour: [u8; 4]) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: 1,
        height: 1,
        depth_or_array_layers: 1,
    };
    let texture = context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("solid source"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    context.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &colour,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

#[test]
fn frame_one_hundred_of_the_assembled_edit_renders_a_picture() {
    let context = match RenderContext::headless() {
        Ok(context) => context,
        Err(RenderError::NoAdapter { backends }) => {
            eprintln!("skipping: no wgpu adapter for backends [{backends}]");
            return;
        }
        Err(error) => panic!("[{}] {error}", error.code()),
    };

    let mut editor = Editor::open("render");
    let assembled = assemble(&mut editor);
    // Render what came back from an undo of everything and a redo of
    // everything, so the picture proves the restored project, not the one the
    // commands left behind.
    while editor.query("history.get")["can_undo"] == Value::Bool(true) {
        editor.query("edit.undo");
    }
    while editor.query("history.get")["can_redo"] == Value::Bool(true) {
        editor.query("edit.redo");
    }
    let project = editor.project();
    editor.close();

    let sequence = &project.sequences[0];
    let red = solid_source(&context, [255, 0, 0, 255]);
    let green = solid_source(&context, [0, 255, 0, 255]);
    let white = solid_source(&context, [255, 255, 255, 255]);

    let device = context.device();
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut compositor = Compositor::for_sequence(context.clone(), sequence);
    let mut source = |resolved: &ResolvedClip<'_>| {
        let view = if resolved.clip_id() == assembled.outgoing_at_100 {
            &red
        } else if resolved.clip_id() == assembled.incoming_at_100 {
            &green
        } else {
            &white
        };
        Some(SourceFrame::new(view.clone(), SOURCE.0, SOURCE.1))
    };
    let summary = compositor.render(sequence, at(RENDER_FRAME), &mut source);
    let pixels = compositor.read_rgba();
    drop(error_scope);

    assert_eq!(summary.drawn(), 2, "the crossfade draws both halves");
    assert!(!summary.is_blank(), "frame {RENDER_FRAME} is not empty");

    // The centre of the canvas, which both halves of the crossfade cover.
    let centre =
        usize::try_from(CANVAS.1 / 2 * CANVAS.0 + CANVAS.0 / 2).expect("a small offset") * 4;
    let pixel = &pixels[centre..centre + 4];
    eprintln!(
        "frame {RENDER_FRAME} centre pixel: {pixel:?} on {}",
        context.describe()
    );
    assert_ne!(
        &pixel[..3],
        &[0, 0, 0],
        "frame {RENDER_FRAME} renders a non-black image, got {pixel:?}",
    );
    // Half way through a red-to-green dissolve, both channels carry light and
    // neither has taken the frame over: a stack drawn in the wrong order or a
    // blend weight of nothing lands on one pure colour instead.
    assert!(
        pixel[0] > 32 && pixel[1] > 32,
        "both halves of the crossfade contribute, got {pixel:?}",
    );
    assert!(
        pixel[2] < 32,
        "nothing but the two halves is on screen, got {pixel:?}",
    );
}
