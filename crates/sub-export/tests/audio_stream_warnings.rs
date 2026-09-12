//! Unknown metadata must not silently suppress multi-stream export warnings.
use gst::prelude::*;
use gstreamer as gst;
use sub_model::{Clip, MediaItem, MediaPath, Project, Sequence, Track, TrackKind};
use sub_time::{Rational, RationalTime, TimeRange};

fn project(path: &str) -> (Project, Sequence) {
    let mut project = Project::new("unprobed audio");
    let item = MediaItem::new(MediaPath::new(path).unwrap());
    let mut track = Track::new("A1", TrackKind::Audio);
    track.items.push(
        Clip::new(
            "take",
            item.id,
            TimeRange::new(
                RationalTime::zero(Rational::FPS_24),
                RationalTime::new(24, Rational::FPS_24),
            )
            .unwrap(),
        )
        .into(),
    );
    project.media.push(item);
    let mut sequence = Sequence::new("cut", sub_model::SequenceSettings::default());
    sequence.tracks.push(track);
    (project, sequence)
}

#[test]
fn an_unprobed_multi_stream_source_warns_without_changing_the_project() {
    gst::init().unwrap();
    for element in ["audiotestsrc", "flacenc", "matroskamux"] {
        assert!(
            gst::ElementFactory::find(element).is_some(),
            "required test element {element}"
        );
    }
    let dir = std::env::temp_dir();
    let name = format!("sub-export-multi-audio-{}.mkv", std::process::id());
    let path = dir.join(&name);
    let launch = format!(
        "matroskamux name=m ! filesink location=\"{}\" audiotestsrc num-buffers=10 ! audioconvert ! flacenc ! m. audiotestsrc num-buffers=10 ! audioconvert ! flacenc ! m.",
        path.display().to_string().replace('\\', "/")
    );
    let pipeline = gst::parse::launch(&launch)
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
    pipeline.set_state(gst::State::Playing).unwrap();
    let message = pipeline
        .bus()
        .unwrap()
        .timed_pop_filtered(
            gst::ClockTime::from_seconds(30),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        )
        .expect("fixture completed");
    pipeline.set_state(gst::State::Null).unwrap();
    assert!(
        matches!(message.view(), gst::MessageView::Eos(_)),
        "{message:?}"
    );
    let (project, sequence) = project(&name);
    let warnings = sub_export::unused_audio_streams_for_export(&project, &sequence, &dir);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("stream 2 unused"), "{warnings:?}");
    assert!(project.media[0].info.is_none());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_failed_probe_reports_that_unused_streams_could_not_be_determined() {
    let (project, sequence) = project("missing-multi-audio.mkv");
    let warnings =
        sub_export::unused_audio_streams_for_export(&project, &sequence, &std::env::temp_dir());
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].contains("could not determine unused audio streams"),
        "{warnings:?}"
    );
}
