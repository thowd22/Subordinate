//! An unprobed video source's audio still goes through GStreamer (TASK-150).
//!
//! decision-4 splits audio decoding in two: GStreamer decodes the audio inside
//! video files, symphonia decodes audio-only files. The split used to be read
//! off the model alone, so a media item that had never been probed — `info` is
//! `None`, the state of an item the bin has not looked at yet or that was
//! offline when the project was saved — was taken for an audio-only file and
//! sent to symphonia. symphonia carries no Opus decoder, so a Matroska file
//! with an Opus track failed every export cell of TASK-143 with
//! `audio.unsupported`, while the same file, once probed, exported without a
//! complaint.
//!
//! The test writes exactly that file — H.264 pictures and an Opus track in
//! Matroska — and renders the same one-clip sequence twice: once from a
//! project whose media item has never been probed, once from a project whose
//! item carries the probe's answer. Both have to come back with the same
//! samples.

use std::path::Path;
use std::sync::Arc;

use sub_export::{
    AudioCodec, AudioFrameSource, Container, ExportElements, ExportSettings, FrameSpan,
    PcmAudioSource, SequenceAudio, VideoCodec, VideoFrameSource, element_is_usable, export_with,
};
use sub_model::{
    AudioStream, Clip, ColorTags, MediaItem, MediaPath, Project, Resolution, Sequence,
    SequenceSettings, StreamInfo, Track, TrackItem, TrackKind, VideoStream,
};
use sub_time::{Rational, RationalTime, TimeRange};

/// Canvas of the fixture: small enough to encode in a moment.
const WIDTH: u32 = 160;
/// Canvas height.
const HEIGHT: u32 = 120;
/// Frames in the fixture: one second at 25 fps.
const FRAMES: u64 = 25;
/// Sample rate of the fixture's tone, which is also the export's.
const SAMPLE_RATE: u32 = 48_000;

/// Grey frames: the picture only has to make the file a video file.
struct GreyFrames {
    pixels: Vec<u8>,
    left: u64,
}

impl VideoFrameSource for GreyFrames {
    fn next_frame(&mut self) -> sub_core::SubResult<Option<&[u8]>> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        Ok(Some(&self.pixels))
    }
}

/// A loud square wave, which survives a lossy codec well enough to be told
/// from silence.
fn tone(samples: usize) -> Vec<f32> {
    let period = SAMPLE_RATE as usize / 440;
    (0..samples)
        .map(|index| {
            if index % period < period / 2 {
                0.5
            } else {
                -0.5
            }
        })
        .collect()
}

/// The peak of a rendered mix, which is what says a decoder actually ran.
fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()))
}

/// Writes the fixture, answering whether this machine could.
fn write_opus_in_matroska(path: &Path, settings: &ExportSettings) -> bool {
    if sub_export::EncoderProbe::cached().is_err() {
        return false;
    }
    let Ok(elements) = ExportElements::resolve(settings, &sub_export::EncoderPreferences::new())
    else {
        return false;
    };
    if !element_is_usable(settings.container.muxer()) {
        return false;
    }
    let mut frames = GreyFrames {
        pixels: vec![0x80; WIDTH as usize * HEIGHT as usize * 4],
        left: FRAMES,
    };
    let count = usize::try_from(settings.audio_frames_through(FRAMES)).expect("a small count");
    let mut audio = PcmAudioSource::new(tone(count));
    export_with(
        path,
        settings,
        &elements,
        &mut frames,
        Some(&mut audio),
        &mut |_| (),
    )
    .expect("the fixture export runs");
    path.is_file()
}

/// A project holding one media item over `name`, with `info` as its probe
/// state, and a sequence with one audio clip over the whole of it.
fn project_over(name: &str, info: Option<StreamInfo>) -> (Project, Sequence) {
    let mut item = MediaItem::new(MediaPath::new(name).expect("a relative media path"));
    item.info = info;
    let media = item.id;
    let mut project = Project::new("Unprobed");
    project.media.push(item);

    let settings = SequenceSettings::new(
        Resolution::new(WIDTH, HEIGHT).expect("a valid canvas"),
        Rational::FPS_25,
        SAMPLE_RATE,
        ColorTags::default(),
    )
    .expect("valid sequence settings");
    let mut sequence = Sequence::new("Main", settings);
    let mut track = Track::new("A1", TrackKind::Audio);
    let range = TimeRange::new(
        RationalTime::zero(Rational::FPS_25),
        RationalTime::new(i64::try_from(FRAMES).expect("small"), Rational::FPS_25),
    )
    .expect("a valid source range");
    track
        .items
        .push(TrackItem::Clip(Clip::new("tone", media, range)));
    sequence.tracks.push(track);
    (project, sequence)
}

/// What the export's mixer makes of `project`'s one clip.
fn render(project: Project, sequence: Sequence, dir: &Path, settings: &ExportSettings) -> Vec<f32> {
    let mut audio = SequenceAudio::new(
        Arc::new(project),
        Arc::new(sequence),
        dir,
        settings,
        FrameSpan::new(0, FRAMES),
    );
    let rendered = audio.render().expect("the mix renders");
    let channels = usize::from(settings.channels);
    let mut samples = vec![0.0f32; rendered.remaining() * channels];
    let mut read = 0;
    while read < samples.len() {
        let taken = rendered
            .read(&mut samples[read..], settings.channels)
            .expect("the rendered mix reads back");
        if taken == 0 {
            break;
        }
        read += taken * channels;
    }
    samples.truncate(read);
    samples
}

#[test]
fn an_unprobed_video_source_decodes_its_opus_audio_like_a_probed_one() {
    let settings = ExportSettings::new(WIDTH, HEIGHT, Rational::FPS_25, Container::Mkv)
        .with_video_codec(VideoCodec::H264)
        // Opus is the codec that found the bug: GStreamer decodes it,
        // symphonia does not carry it at all.
        .with_audio_codec(Some(AudioCodec::Opus))
        .with_audio_format(SAMPLE_RATE, 1);

    let dir = std::env::temp_dir().join(format!("sub-export-unprobed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    let path = dir.join("tone.mkv");
    if !write_opus_in_matroska(&path, &settings) {
        eprintln!("skipping: this machine has no usable H.264 encoder, opusenc or matroskamux");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    // Never probed: `info` is None, which used to route this video file to
    // symphonia and fail with `audio.unsupported`.
    let (project, sequence) = project_over("tone.mkv", None);
    let unprobed = render(project, sequence, &dir, &settings);
    assert!(
        peak(&unprobed) > 0.05,
        "an unprobed video source renders its audio, not silence: peak {}",
        peak(&unprobed)
    );

    // Probed: the same file, with the answer the model would have carried.
    let info = StreamInfo {
        duration: Some(RationalTime::new(
            i64::try_from(FRAMES).expect("small"),
            Rational::FPS_25,
        )),
        video: vec![VideoStream {
            width: WIDTH,
            height: HEIGHT,
            frame_rate: Rational::FPS_25,
            sample_aspect: Rational::ONE,
            color: ColorTags::default(),
        }],
        audio: vec![AudioStream {
            channels: 1,
            sample_rate: SAMPLE_RATE,
        }],
    };
    let (project, sequence) = project_over("tone.mkv", Some(info));
    let probed = render(project, sequence, &dir, &settings);

    assert_eq!(
        unprobed.len(),
        probed.len(),
        "an unprobed source renders the same number of samples as a probed one"
    );
    assert_eq!(
        unprobed, probed,
        "whether the media had been probed does not change what the export hears"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
