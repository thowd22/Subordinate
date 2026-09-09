//! Golden test for the project file format.
//!
//! `fixtures/sample-project.sub` is a committed, realistic project: two
//! sequences of three tracks each, clips, a crossfade, markers and two bins
//! under the root bin. Two properties are checked against it.
//!
//! - **Round-trip**: loading the fixture and saving it again produces the same
//!   bytes, so the format is stable and a `.sub` file survives an open/save
//!   with an empty git diff (docs/PLAN.md §5.6).
//! - **Golden**: [`sample_project`], built in code with fixed identifiers,
//!   serialises to exactly the committed bytes. Any change to field names,
//!   ordering, defaults or encodings fails here with a line diff.
//!
//! When a change to the format is intended, regenerate the fixture with
//! `SUB_UPDATE_GOLDEN=1 cargo test -p sub-model --test golden` and commit the
//! result: the diff of that file is the review of the format change.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sub_model::content::{ContentHash, MediaPath};
use sub_model::marker::Marker;
use sub_model::media::{AudioStream, Bin, MediaItem, ProxyState, StreamInfo, VideoStream};
use sub_model::params::{Fixed6, GainDb, Opacity, Point2, Scale2, Transform};
use sub_model::sequence::{ColorTags, Resolution, Sequence, SequenceSettings};
use sub_model::track::{Clip, Gap, Track, TrackKind, Transition};
use sub_model::{BinId, ClipId, MarkerId, MediaId, Project, ProjectId, SequenceId, TrackId, json};
use sub_time::{Rational, RationalTime, TimeRange};

/// Environment variable that rewrites the committed fixture instead of
/// comparing against it.
const UPDATE_ENV: &str = "SUB_UPDATE_GOLDEN";

/// How the failure message tells the reader to accept an intended change.
const REGENERATE_HINT: &str = "if this change to the project file format is intended, regenerate \
                               the fixture with `SUB_UPDATE_GOLDEN=1 cargo test -p sub-model \
                               --test golden` and commit the diff";

/// The committed sample project.
fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample-project.sub")
}

/// The bytes of the committed sample project, as text.
fn fixture_text() -> String {
    let path = fixture_path();
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("could not read {}: {err}", path.display()))
}

/// Parses a fixed identifier, so the fixture bytes do not change from run to
/// run the way freshly minted `UUIDv7`s would.
macro_rules! id {
    ($ty:ty, $text:literal) => {
        <$ty>::parse($text).expect("fixed fixture id")
    };
}

/// Builds the sample project: two sequences with three tracks each, clips, a
/// crossfade, markers and two bins.
///
/// Every identifier is fixed and every time is exact, so the serialised bytes
/// are a pure function of this code.
#[expect(
    clippy::too_many_lines,
    reason = "one flat fixture builder is easier to read against the JSON than a \
              scatter of helpers"
)]
fn sample_project() -> Project {
    let uhd = Rational::FPS_23_976;
    let hd = Rational::FPS_25;

    let mut project = Project::new("Doc cut");
    project.id = id!(ProjectId, "0192f3a0-0000-7000-8000-000000000001");
    project.root_bin.id = id!(BinId, "0192f3a0-0000-7000-8000-000000000002");

    // --- Media -----------------------------------------------------------
    let interview_id = id!(MediaId, "0192f3a0-0001-7000-8000-000000000001");
    let mut interview = MediaItem::new(MediaPath::new("footage/interview.mp4").expect("path"));
    interview.id = interview_id;
    "Interview A".clone_into(&mut interview.name);
    interview.hash = Some(ContentHash::from_bytes([0x11; 32]));
    interview.info = Some(StreamInfo {
        duration: Some(RationalTime::new(24_024, uhd)),
        video: vec![VideoStream {
            width: 3840,
            height: 2160,
            frame_rate: uhd,
            sample_aspect: Rational::ONE,
            color: ColorTags::REC709,
        }],
        audio: vec![AudioStream {
            channels: 2,
            sample_rate: 48_000,
        }],
    });
    interview.proxy =
        ProxyState::Ready(MediaPath::new("project.sub.d/proxy/interview.mov").expect("path"));

    let broll_id = id!(MediaId, "0192f3a0-0001-7000-8000-000000000002");
    let mut broll = MediaItem::new(MediaPath::new("footage/broll-city.mov").expect("path"));
    broll.id = broll_id;
    "City b-roll".clone_into(&mut broll.name);
    broll.hash = Some(ContentHash::from_bytes([0x22; 32]));
    broll.proxy = ProxyState::Pending;

    let tone_id = id!(MediaId, "0192f3a0-0001-7000-8000-000000000003");
    let mut tone = MediaItem::new(MediaPath::new("audio/room-tone.wav").expect("path"));
    tone.id = tone_id;
    "Room tone".clone_into(&mut tone.name);
    tone.offline = true;
    tone.proxy = ProxyState::Failed("no audio proxy encoder".to_owned());

    project.media.push(interview);
    project.media.push(broll);
    project.media.push(tone);

    // --- Bins ------------------------------------------------------------
    let mut interviews = Bin::new("Interviews");
    interviews.id = id!(BinId, "0192f3a0-0002-7000-8000-000000000001");
    interviews.media.push(interview_id);

    let mut broll_bin = Bin::new("B-roll");
    broll_bin.id = id!(BinId, "0192f3a0-0002-7000-8000-000000000002");
    broll_bin.media.push(broll_id);

    project.root_bin.media.push(tone_id);
    project.root_bin.children.push(interviews);
    project.root_bin.children.push(broll_bin);

    // --- Sequence 1: "Main" ---------------------------------------------
    let source = |start: i64, duration: i64, rate: Rational| {
        TimeRange::new(
            RationalTime::new(start, rate),
            RationalTime::new(duration, rate),
        )
        .expect("non-negative fixture range")
    };

    let mut shot_one = Clip::new("shot 1", interview_id, source(24, 96, uhd));
    shot_one.id = id!(ClipId, "0192f3a0-0003-7000-8000-000000000001");
    shot_one.opacity = Opacity::new(Fixed6::from_micros(900_000)).expect("opacity in range");
    shot_one.gain = GainDb::new(Fixed6::from_micros(-3_000_000)).expect("gain in range");
    shot_one.fade_in = RationalTime::new(6, uhd);
    let mut look_here = Marker::new("look here", TimeRange::empty_at(RationalTime::new(30, uhd)));
    look_here.id = id!(MarkerId, "0192f3a0-0004-7000-8000-000000000001");
    "cut on the nod".clone_into(&mut look_here.note);
    shot_one.markers.push(look_here);

    let mut shot_two = Clip::new("shot 2", broll_id, source(0, 72, uhd));
    shot_two.id = id!(ClipId, "0192f3a0-0003-7000-8000-000000000002");
    shot_two.transform = Transform::new(
        Point2::new(Fixed6::from_units(12), Fixed6::from_units(-8)),
        Scale2::uniform(Fixed6::from_micros(1_100_000)).expect("scale in range"),
        Fixed6::from_micros(2_500_000),
    );
    shot_two.fade_out = RationalTime::new(12, uhd);

    let mut video_a = Track::new("V1", TrackKind::Video);
    video_a.id = id!(TrackId, "0192f3a0-0005-7000-8000-000000000001");
    video_a.items.push(shot_one.into());
    video_a
        .items
        .push(Transition::crossfade(RationalTime::new(6, uhd), RationalTime::new(6, uhd)).into());
    video_a.items.push(shot_two.into());

    let mut lower_third = Clip::new("lower third", broll_id, source(120, 48, uhd));
    lower_third.id = id!(ClipId, "0192f3a0-0003-7000-8000-000000000003");
    lower_third.opacity = Opacity::new(Fixed6::from_micros(750_000)).expect("opacity in range");

    let mut video_b = Track::new("V2", TrackKind::Video);
    video_b.id = id!(TrackId, "0192f3a0-0005-7000-8000-000000000002");
    video_b
        .items
        .push(Gap::new(RationalTime::new(24, uhd)).into());
    video_b.items.push(lower_third.into());

    let mut room_tone = Clip::new("room tone", tone_id, source(0, 168, uhd));
    room_tone.id = id!(ClipId, "0192f3a0-0003-7000-8000-000000000004");
    room_tone.gain = GainDb::new(Fixed6::from_micros(-12_000_000)).expect("gain in range");
    room_tone.fade_in = RationalTime::new(12, uhd);
    room_tone.fade_out = RationalTime::new(12, uhd);

    let mut audio_a = Track::new("A1", TrackKind::Audio);
    audio_a.id = id!(TrackId, "0192f3a0-0005-7000-8000-000000000003");
    audio_a.items.push(room_tone.into());

    let mut main = Sequence::new(
        "Main",
        SequenceSettings::new(Resolution::UHD_2160, uhd, 48_000, ColorTags::REC709)
            .expect("non-zero sample rate"),
    );
    main.id = id!(SequenceId, "0192f3a0-0006-7000-8000-000000000001");
    main.tracks.push(video_a);
    main.tracks.push(video_b);
    main.tracks.push(audio_a);
    let mut act_two = Marker::new(
        "act two",
        TimeRange::new(RationalTime::new(96, uhd), RationalTime::new(24, uhd)).expect("range"),
    );
    act_two.id = id!(MarkerId, "0192f3a0-0004-7000-8000-000000000002");
    main.markers.push(act_two);

    // --- Sequence 2: "Titles" -------------------------------------------
    let mut title_card = Clip::new("title card", broll_id, source(0, 50, hd));
    title_card.id = id!(ClipId, "0192f3a0-0003-7000-8000-000000000005");
    title_card.fade_in = RationalTime::new(10, hd);

    let mut card_track = Track::new("V1", TrackKind::Video);
    card_track.id = id!(TrackId, "0192f3a0-0005-7000-8000-000000000004");
    card_track.items.push(title_card.into());

    let mut overlay_track = Track::new("V2", TrackKind::Video);
    overlay_track.id = id!(TrackId, "0192f3a0-0005-7000-8000-000000000005");
    overlay_track
        .items
        .push(Gap::new(RationalTime::new(50, hd)).into());

    let mut sting_track = Track::new("A1", TrackKind::Audio);
    sting_track.id = id!(TrackId, "0192f3a0-0005-7000-8000-000000000006");
    let mut sting = Clip::new("sting", tone_id, source(25, 25, hd));
    sting.id = id!(ClipId, "0192f3a0-0003-7000-8000-000000000006");
    sting_track.items.push(sting.into());

    let mut titles = Sequence::new(
        "Titles",
        SequenceSettings::new(Resolution::HD_1080, hd, 48_000, ColorTags::REC709)
            .expect("non-zero sample rate"),
    );
    titles.id = id!(SequenceId, "0192f3a0-0006-7000-8000-000000000002");
    titles.tracks.push(card_track);
    titles.tracks.push(overlay_track);
    titles.tracks.push(sting_track);
    let mut logo = Marker::new("logo out", TimeRange::empty_at(RationalTime::new(45, hd)));
    logo.id = id!(MarkerId, "0192f3a0-0004-7000-8000-000000000003");
    titles.markers.push(logo);

    project.sequences.push(main);
    project.sequences.push(titles);
    project
}

/// A readable line diff of two project files.
///
/// Line-based rather than character-based because the file is pretty-printed
/// one value per line: the first differing line names the field that changed,
/// and a few lines of context show which object it sits in.
fn diff(expected: &str, actual: &str) -> String {
    const CONTEXT: usize = 3;

    let expected: Vec<&str> = expected.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();
    let Some(first) =
        (0..expected.len().max(actual.len())).find(|&i| expected.get(i) != actual.get(i))
    else {
        return "no differing lines (the files differ only in trailing newlines)".to_owned();
    };

    let mut report = String::new();
    let _ = writeln!(report, "first difference at line {}", first + 1);
    for line in &expected[first.saturating_sub(CONTEXT)..first] {
        let _ = writeln!(report, "  {line}");
    }
    match expected.get(first) {
        Some(line) => {
            let _ = writeln!(report, "- {line}");
        }
        None => report.push_str("- (end of committed fixture)\n"),
    }
    match actual.get(first) {
        Some(line) => {
            let _ = writeln!(report, "+ {line}");
        }
        None => report.push_str("+ (end of generated output)\n"),
    }
    for line in
        &expected[(first + 1).min(expected.len())..(first + 1 + CONTEXT).min(expected.len())]
    {
        let _ = writeln!(report, "  {line}");
    }
    let _ = writeln!(
        report,
        "{} lines committed, {} lines generated\n{REGENERATE_HINT}",
        expected.len(),
        actual.len()
    );
    report
}

#[test]
fn the_sample_project_saves_to_the_committed_bytes() {
    let generated = json::to_json(&sample_project()).expect("the sample project serialises");
    if std::env::var_os(UPDATE_ENV).is_some() {
        std::fs::write(fixture_path(), &generated).expect("the fixture is writable");
        return;
    }
    let committed = fixture_text();
    assert!(
        committed == generated,
        "the project file format changed\n{}",
        diff(&committed, &generated)
    );
}

#[test]
fn the_committed_sample_project_round_trips_byte_identically() {
    if std::env::var_os(UPDATE_ENV).is_some() {
        // The sibling test is rewriting the fixture in parallel; there is
        // nothing stable on disk to compare against in this run.
        return;
    }
    let committed = fixture_text();
    let project = json::from_json(&committed).expect("the committed fixture loads");
    let saved = json::to_json(&project).expect("the loaded project saves");
    assert!(
        committed == saved,
        "loading and saving the fixture changed its bytes\n{}",
        diff(&committed, &saved)
    );
    // A second trip proves the first was not a lucky normalisation.
    let reloaded = json::from_json(&saved).expect("the saved project loads");
    assert_eq!(reloaded, project);
    assert_eq!(json::to_json(&reloaded).expect("stable"), committed);
}

#[test]
fn the_committed_sample_project_holds_what_the_fixture_promises() {
    if std::env::var_os(UPDATE_ENV).is_some() {
        return;
    }
    let project = json::from_json(&fixture_text()).expect("the committed fixture loads");

    assert_eq!(project.sequences.len(), 2, "two sequences");
    for sequence in &project.sequences {
        assert_eq!(
            sequence.tracks.len(),
            3,
            "three tracks in {}",
            sequence.name
        );
        assert!(
            sequence
                .tracks
                .iter()
                .any(|track| track.clips().count() > 0),
            "clips in {}",
            sequence.name
        );
        assert!(!sequence.markers.is_empty(), "markers in {}", sequence.name);
    }

    let crossfades = project
        .sequences
        .iter()
        .flat_map(|sequence| &sequence.tracks)
        .flat_map(|track| &track.items)
        .filter(|item| matches!(item, sub_model::TrackItem::Transition(_)))
        .count();
    assert_eq!(crossfades, 1, "one crossfade");

    assert_eq!(project.root_bin.children.len(), 2, "two bins");
    assert_eq!(project.media.len(), 3, "three media items");
    for item in &project.media {
        assert!(
            project.root_bin.contains_media(item.id)
                || project
                    .root_bin
                    .children
                    .iter()
                    .any(|bin| bin.contains_media(item.id)),
            "{} is filed in a bin",
            item.name
        );
    }
    assert!(
        project
            .sequences
            .iter()
            .flat_map(|sequence| &sequence.tracks)
            .flat_map(sub_model::Track::clips)
            .any(|clip| !clip.markers.is_empty()),
        "a clip marker"
    );
}

#[test]
fn the_diff_names_the_line_that_changed() {
    let committed = json::to_json(&sample_project()).expect("serialises");
    let changed = committed.replacen("\"name\": \"Main\"", "\"name\": \"Maim\"", 1);
    assert_ne!(changed, committed, "the test edit applied");

    let report = diff(&committed, &changed);
    assert!(report.contains("- "), "{report}");
    assert!(report.contains("+ "), "{report}");
    assert!(report.contains("\"Main\""), "{report}");
    assert!(report.contains("\"Maim\""), "{report}");
    assert!(report.contains("first difference at line"), "{report}");
    assert!(report.contains(UPDATE_ENV), "{report}");
}

#[test]
fn the_diff_reports_a_truncated_file() {
    let committed = json::to_json(&sample_project()).expect("serialises");
    let mut truncated = String::new();
    for line in committed.lines().take(5) {
        let _ = writeln!(truncated, "{line}");
    }
    let report = diff(&committed, &truncated);
    assert!(report.contains("end of generated output"), "{report}");
}
