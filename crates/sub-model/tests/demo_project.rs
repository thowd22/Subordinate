//! The sample project shipped in `examples/sample-project` (TASK-109).
//!
//! `examples/sample-project/demo.sub` is the project a new user — or an agent
//! — opens to see the editor doing something: two sequences, clips over three
//! tracks, a crossfade, sequence and clip markers, and three real CC0 clips
//! fetched by `scripts/get-sample-media.sh`. `crates/sub-ui/tests/
//! sample_project_render.rs` is the render test that decodes that media and
//! composites it.
//!
//! This file is the project's *source*: [`demo_project`] builds it in code
//! with fixed identifiers, and the tests below hold the committed file to it,
//! exactly as `golden.rs` does for the format fixture. Two further properties
//! matter here and not there, because this project names files that really
//! exist:
//!
//! - every media path is **relative and forward-slashed**, so the file opens
//!   without a relink on Linux, Windows and macOS alike (`MediaPath::resolve`
//!   builds the native path at load time);
//! - every media item records the **content hash** of the pinned download, so
//!   media that is not the media this project was authored against is caught
//!   rather than silently rendered.
//!
//! When a change to the sample project is intended, regenerate it with
//! `SUB_UPDATE_GOLDEN=1 cargo test -p sub-model --test demo_project` and commit
//! the diff.

use std::path::{Path, PathBuf};

use sub_model::content::{ContentHash, MediaPath};
use sub_model::marker::Marker;
use sub_model::media::{AudioStream, Bin, MediaItem, StreamInfo, VideoStream};
use sub_model::params::{Fixed6, GainDb, Opacity, Point2, Scale2, Transform};
use sub_model::sequence::{ColorTags, Resolution, Sequence, SequenceSettings};
use sub_model::track::{Clip, Gap, Track, TrackKind, Transition};
use sub_model::{BinId, ClipId, MarkerId, MediaId, Project, ProjectId, SequenceId, TrackId, json};
use sub_time::{Rational, RationalTime, TimeRange};

/// Environment variable that rewrites the committed project instead of
/// comparing against it.
const UPDATE_ENV: &str = "SUB_UPDATE_GOLDEN";

/// How the failure message tells the reader to accept an intended change.
const REGENERATE_HINT: &str = "if this change to the sample project is intended, regenerate it \
                               with `SUB_UPDATE_GOLDEN=1 cargo test -p sub-model --test \
                               demo_project` and commit the diff";

/// The committed sample project.
fn project_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sample-project/demo.sub")
}

/// Parses a fixed identifier, so the bytes do not change from run to run the
/// way freshly minted `UUIDv7`s would.
macro_rules! id {
    ($ty:ty, $text:literal) => {
        <$ty>::parse($text).expect("fixed sample id")
    };
}

/// Rate a media duration is expressed in: whole nanoseconds, which is what the
/// probe reports and what keeps a 89/3 fps source exact.
fn nanos() -> Rational {
    Rational::new(1_000_000_000, 1).expect("a non-zero rate")
}

/// A range from a start and a duration in `rate`'s units.
fn range(start: i64, duration: i64, rate: Rational) -> TimeRange {
    TimeRange::new(
        RationalTime::new(start, rate),
        RationalTime::new(duration, rate),
    )
    .expect("a non-negative sample range")
}

/// The sample project, as `examples/sample-project/demo.sub` holds it.
///
/// The three media items are the three CC0 files
/// `scripts/get-sample-media.sh` downloads, with the stream information the
/// probe reports for them and the content hash of the pinned bytes. Both
/// sequences cut only inside those durations, so nothing on either timeline
/// reaches past the end of its source.
#[expect(
    clippy::too_many_lines,
    reason = "one flat builder is easier to read against the JSON than a scatter of helpers"
)]
fn demo_project() -> Project {
    let ns = nanos();
    let main_rate = Rational::FPS_25;
    let title_rate = Rational::FPS_30;

    let mut project = Project::new("Subordinate sample");
    project.id = id!(ProjectId, "0193a1b0-0000-7000-8000-000000000001");
    project.root_bin.id = id!(BinId, "0193a1b0-0000-7000-8000-000000000002");

    // --- Media -----------------------------------------------------------
    // "Ancienne et nouvelle tenue des porteurs des Pompes Funebres de la
    // Ville de Paris" (1921), Le Saint Lucien, CC0. 720x576 with a 16:15
    // sample aspect, so the sample project also exercises non-square pixels.
    let porters_id = id!(MediaId, "0193a1b0-0001-7000-8000-000000000001");
    let mut porters =
        MediaItem::new(MediaPath::new("media/porters-paris-1921.webm").expect("path"));
    porters.id = porters_id;
    "Porters, Paris 1921".clone_into(&mut porters.name);
    porters.hash = Some(
        ContentHash::parse("dd9f1341ecb4f14ed39aaa7f498c8c166da0d70240869c8ff166d4e1fbd8f3c3")
            .expect("a hash of the pinned download"),
    );
    porters.info = Some(StreamInfo {
        duration: Some(RationalTime::new(17_173_000_000, ns)),
        video: vec![VideoStream {
            width: 720,
            height: 576,
            frame_rate: Rational::FPS_30,
            sample_aspect: Rational::new(16, 15).expect("a non-zero aspect"),
            color: ColorTags::REC709,
        }],
        audio: vec![AudioStream {
            channels: 2,
            sample_rate: 48_000,
        }],
    });

    // "Victorian Crowned Pigeon fighting" (2023), Designism, CC0.
    let pigeon_id = id!(MediaId, "0193a1b0-0001-7000-8000-000000000002");
    let mut pigeon = MediaItem::new(MediaPath::new("media/crowned-pigeon.webm").expect("path"));
    pigeon.id = pigeon_id;
    "Crowned pigeon".clone_into(&mut pigeon.name);
    pigeon.hash = Some(
        ContentHash::parse("e5baaa1a2bfc0ade53dd4a9efbcbc43f46dd0d0113310b64b3dd0920fa218e22")
            .expect("a hash of the pinned download"),
    );
    pigeon.info = Some(StreamInfo {
        duration: Some(RationalTime::new(24_248_000_000, ns)),
        video: vec![VideoStream {
            width: 1010,
            height: 616,
            frame_rate: Rational::new(89, 3).expect("a non-zero rate"),
            sample_aspect: Rational::ONE,
            color: ColorTags::REC709,
        }],
        audio: vec![AudioStream {
            channels: 2,
            sample_rate: 48_000,
        }],
    });

    // "Soneros en Xalapa" (2013), Koffermejia, CC0. The small one, and the
    // only mono source.
    let soneros_id = id!(MediaId, "0193a1b0-0001-7000-8000-000000000003");
    let mut soneros = MediaItem::new(MediaPath::new("media/soneros-en-xalapa.webm").expect("path"));
    soneros.id = soneros_id;
    "Soneros en Xalapa".clone_into(&mut soneros.name);
    soneros.hash = Some(
        ContentHash::parse("3f3ea88176e0177622e11f7997bec33ead16cc340e04d25382f3a91ac52e8b50")
            .expect("a hash of the pinned download"),
    );
    soneros.info = Some(StreamInfo {
        duration: Some(RationalTime::new(5_197_000_000, ns)),
        video: vec![VideoStream {
            width: 352,
            height: 288,
            frame_rate: Rational::new(25, 3).expect("a non-zero rate"),
            sample_aspect: Rational::ONE,
            color: ColorTags::default(),
        }],
        audio: vec![AudioStream {
            channels: 1,
            sample_rate: 8_000,
        }],
    });

    project.media.push(porters);
    project.media.push(pigeon);
    project.media.push(soneros);

    // --- Bins ------------------------------------------------------------
    let mut footage = Bin::new("Footage");
    footage.id = id!(BinId, "0193a1b0-0002-7000-8000-000000000001");
    footage.media.push(porters_id);
    footage.media.push(pigeon_id);
    project.root_bin.children.push(footage);
    project.root_bin.media.push(soneros_id);

    // --- Sequence 1: "Main cut", 1280x720 at 25 fps ----------------------
    //
    // V1 runs two clips into a crossfade; V2 lays the small clip over the
    // second half at 60 % opacity, scaled down and pushed into the top right;
    // A1 carries the music bed under both.
    let mut porters_shot = Clip::new("Porters, wide", porters_id, range(50, 150, main_rate));
    porters_shot.id = id!(ClipId, "0193a1b0-0003-7000-8000-000000000001");
    porters_shot.fade_in = RationalTime::new(12, main_rate);
    let mut wide = Marker::new(
        "wide shot",
        TimeRange::empty_at(RationalTime::new(70, main_rate)),
    );
    wide.id = id!(MarkerId, "0193a1b0-0004-7000-8000-000000000001");
    "the two uniforms are both in frame here".clone_into(&mut wide.note);
    porters_shot.markers.push(wide);

    let mut pigeon_shot = Clip::new("Pigeon, fight", pigeon_id, range(75, 200, main_rate));
    pigeon_shot.id = id!(ClipId, "0193a1b0-0003-7000-8000-000000000002");
    pigeon_shot.fade_out = RationalTime::new(25, main_rate);

    let mut programme = Track::new("V1", TrackKind::Video);
    programme.id = id!(TrackId, "0193a1b0-0005-7000-8000-000000000001");
    programme.items.push(porters_shot.into());
    programme.items.push(
        Transition::crossfade(
            RationalTime::new(12, main_rate),
            RationalTime::new(12, main_rate),
        )
        .into(),
    );
    programme.items.push(pigeon_shot.into());

    let mut overlay_clip = Clip::new("Soneros, corner", soneros_id, range(0, 100, main_rate));
    overlay_clip.id = id!(ClipId, "0193a1b0-0003-7000-8000-000000000003");
    overlay_clip.opacity = Opacity::new(Fixed6::from_micros(600_000)).expect("opacity in range");
    overlay_clip.transform = Transform::new(
        Point2::new(Fixed6::from_units(320), Fixed6::from_units(-160)),
        Scale2::uniform(Fixed6::from_micros(450_000)).expect("scale in range"),
        Fixed6::from_units(0),
    );

    let mut overlay = Track::new("V2", TrackKind::Video);
    overlay.id = id!(TrackId, "0193a1b0-0005-7000-8000-000000000002");
    overlay
        .items
        .push(Gap::new(RationalTime::new(100, main_rate)).into());
    overlay.items.push(overlay_clip.into());

    let mut music_clip = Clip::new("Soneros, music bed", soneros_id, range(0, 125, main_rate));
    music_clip.id = id!(ClipId, "0193a1b0-0003-7000-8000-000000000004");
    music_clip.gain = GainDb::new(Fixed6::from_micros(-6_000_000)).expect("gain in range");
    music_clip.fade_in = RationalTime::new(12, main_rate);
    music_clip.fade_out = RationalTime::new(12, main_rate);

    let mut music = Track::new("A1", TrackKind::Audio);
    music.id = id!(TrackId, "0193a1b0-0005-7000-8000-000000000003");
    music.items.push(music_clip.into());

    let mut main = Sequence::new(
        "Main cut",
        SequenceSettings::new(
            Resolution::new(1280, 720).expect("a non-zero resolution"),
            main_rate,
            48_000,
            ColorTags::REC709,
        )
        .expect("non-zero sample rate"),
    );
    main.id = id!(SequenceId, "0193a1b0-0006-7000-8000-000000000001");
    main.tracks.push(programme);
    main.tracks.push(overlay);
    main.tracks.push(music);
    let mut dissolve = Marker::new(
        "dissolve",
        TimeRange::new(
            RationalTime::new(138, main_rate),
            RationalTime::new(24, main_rate),
        )
        .expect("a non-negative marker range"),
    );
    dissolve.id = id!(MarkerId, "0193a1b0-0004-7000-8000-000000000002");
    "one second of crossfade, centred on the cut".clone_into(&mut dissolve.note);
    main.markers.push(dissolve);

    // --- Sequence 2: "Titles", 1920x1080 at 30 fps -----------------------
    let mut title_bed = Clip::new("Pigeon, title bed", pigeon_id, range(360, 90, title_rate));
    title_bed.id = id!(ClipId, "0193a1b0-0003-7000-8000-000000000005");
    title_bed.fade_in = RationalTime::new(15, title_rate);
    title_bed.fade_out = RationalTime::new(15, title_rate);

    let mut bed_track = Track::new("V1", TrackKind::Video);
    bed_track.id = id!(TrackId, "0193a1b0-0005-7000-8000-000000000004");
    bed_track.items.push(title_bed.into());

    let mut sting_clip = Clip::new("Soneros, sting", soneros_id, range(0, 90, title_rate));
    sting_clip.id = id!(ClipId, "0193a1b0-0003-7000-8000-000000000006");
    sting_clip.gain = GainDb::new(Fixed6::from_micros(-3_000_000)).expect("gain in range");

    let mut sting_track = Track::new("A1", TrackKind::Audio);
    sting_track.id = id!(TrackId, "0193a1b0-0005-7000-8000-000000000005");
    sting_track.items.push(sting_clip.into());

    let mut titles = Sequence::new(
        "Titles",
        SequenceSettings::new(Resolution::HD_1080, title_rate, 48_000, ColorTags::REC709)
            .expect("non-zero sample rate"),
    );
    titles.id = id!(SequenceId, "0193a1b0-0006-7000-8000-000000000002");
    titles.tracks.push(bed_track);
    titles.tracks.push(sting_track);
    let mut logo = Marker::new(
        "logo out",
        TimeRange::empty_at(RationalTime::new(75, title_rate)),
    );
    logo.id = id!(MarkerId, "0193a1b0-0004-7000-8000-000000000003");
    titles.markers.push(logo);

    project.sequences.push(main);
    project.sequences.push(titles);
    project
}

/// The committed project file, as text.
fn committed_text() -> String {
    let path = project_path();
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("could not read {}: {err}", path.display()))
}

/// The first line at which two files differ, for a readable failure.
fn first_difference(expected: &str, actual: &str) -> String {
    let expected: Vec<&str> = expected.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();
    match (0..expected.len().max(actual.len())).find(|&i| expected.get(i) != actual.get(i)) {
        Some(i) => format!(
            "first difference at line {}\n- {}\n+ {}",
            i + 1,
            expected.get(i).unwrap_or(&"(end of committed file)"),
            actual.get(i).unwrap_or(&"(end of generated file)"),
        ),
        None => "no differing lines (the files differ only in trailing newlines)".to_owned(),
    }
}

#[test]
fn the_demo_project_saves_to_the_committed_bytes() {
    let generated = json::to_json(&demo_project()).expect("the sample project serialises");
    if std::env::var_os(UPDATE_ENV).is_some() {
        std::fs::write(project_path(), &generated).expect("the sample project is writable");
        return;
    }
    let committed = committed_text();
    assert!(
        committed == generated,
        "the sample project changed\n{}\n{REGENERATE_HINT}",
        first_difference(&committed, &generated)
    );
}

#[test]
fn the_committed_demo_project_round_trips_byte_identically() {
    let committed = committed_text();
    let loaded = json::from_json(&committed).expect("the sample project loads");
    let saved = json::to_json(&loaded).expect("the sample project saves");
    assert!(
        committed == saved,
        "the sample project did not survive a load and save\n{}",
        first_difference(&committed, &saved)
    );
}

#[test]
fn every_media_path_is_relative_and_forward_slashed() {
    let project = json::from_json(&committed_text()).expect("the sample project loads");
    let project_dir = project_path()
        .parent()
        .expect("the project has a directory")
        .to_path_buf();
    assert_eq!(project.media.len(), 3);
    for item in &project.media {
        let text = item.path.as_str();
        assert!(
            !text.contains('\\'),
            "{text} carries a backslash, which is not a path separator in a project file"
        );
        assert!(
            !text.starts_with('/') && !text.contains(':'),
            "{text} is absolute; a sample project that is checked out anywhere must be relative"
        );
        assert!(
            text.starts_with("media/"),
            "{text} does not live in the directory scripts/get-sample-media.sh writes"
        );
        // The path the loader would open on this OS. It has to name a file the
        // fetch script produces, whatever the native separator is.
        let resolved = item.path.resolve(&project_dir);
        assert_eq!(
            resolved.parent().expect("a media directory"),
            project_dir.join("media"),
        );
        assert!(item.hash.is_some(), "{text} records no content hash");
    }
}

#[test]
fn the_demo_project_holds_what_the_sample_promises() {
    let project = json::from_json(&committed_text()).expect("the sample project loads");
    assert_eq!(project.sequences.len(), 2, "two sequences");

    let main = &project.sequences[0];
    assert_eq!(main.name, "Main cut");
    assert_eq!(main.tracks.len(), 3, "V1, V2 and A1");
    let clips = main
        .tracks
        .iter()
        .flat_map(|track| track.items.iter())
        .filter(|item| item.as_clip().is_some())
        .count();
    assert_eq!(clips, 4, "four clips on the main sequence");
    let transitions = main
        .tracks
        .iter()
        .flat_map(|track| track.items.iter())
        .filter(|item| item.as_transition().is_some())
        .count();
    assert_eq!(transitions, 1, "one crossfade");
    assert_eq!(main.markers.len(), 1, "one sequence marker");
    let clip_markers: usize = main
        .tracks
        .iter()
        .flat_map(|track| track.items.iter())
        .filter_map(sub_model::track::TrackItem::as_clip)
        .map(|clip| clip.markers.len())
        .sum();
    assert_eq!(clip_markers, 1, "one clip marker");

    let titles = &project.sequences[1];
    assert_eq!(titles.name, "Titles");
    assert_eq!(titles.markers.len(), 1);
}
