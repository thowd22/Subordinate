//! Real decoded samples through the timeline's callback mixer, without a device.
use std::path::PathBuf;
use std::time::{Duration, Instant};

use sub_edit::History;
use sub_model::media::{AudioStream, StreamInfo};
use sub_model::{MediaItem, MediaPath, Project, Sequence, SequenceSettings, Track, TrackKind};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::playback_audio::PlaybackAudio;
use sub_ui::source_edit::{EditMode, apply_source_edit, plan_timeline_source_edit};

fn seconds(value: i64) -> RationalTime {
    RationalTime::new(value, Rational::ONE)
}

struct Wav(PathBuf);
impl Wav {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("sub-playback-audio-{}.wav", std::process::id()));
        let frames = 480_000_u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&96_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(frames * 2).to_le_bytes());
        for frame in 0..frames {
            let sample: i16 = if frame < 96_000 { 8192 } else { -8192 };
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }
}
impl Drop for Wav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn await_audio(audio: &mut PlaybackAudio, position: RationalTime) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        audio.update(position);
        assert!(audio.error().is_none(), "{:?}", audio.error());
        if audio.ready() && !audio.loading() {
            return;
        }
        assert!(Instant::now() < deadline, "worker did not prepare audio");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn source_drop_decodes_sound_and_seeking_uses_source_inpoint_at_one_x() {
    sub_media::init().unwrap();
    let wav = Wav::new();
    let mut item = MediaItem::new(MediaPath::external(&wav.0).unwrap());
    item.info = Some(StreamInfo {
        duration: Some(seconds(10)),
        video: Vec::new(),
        audio: vec![AudioStream {
            channels: 1,
            sample_rate: 48_000,
        }],
    });
    let media = item.id;
    let mut project = Project::new("Audio playback");
    project.media.push(item);
    let mut sequence = Sequence::new("Main", SequenceSettings::default());
    sequence.tracks.push(Track::new("A1", TrackKind::Audio));
    project.sequences.push(sequence.clone());
    let plan = plan_timeline_source_edit(
        &project,
        &sequence,
        media,
        0,
        seconds(2),
        EditMode::Overwrite,
    )
    .unwrap();
    apply_source_edit(&mut History::new(), &mut project, plan).unwrap();
    let sequence = &project.sequences[0];
    let mut audio = PlaybackAudio::new(48_000).unwrap();
    audio
        .configure(&project, sequence, wav.0.parent().unwrap())
        .unwrap();
    assert!(
        !audio.loading(),
        "configuring must not eagerly decode the playlist"
    );
    await_audio(&mut audio, seconds(2));
    let (mut control, mut mixer) = audio.create_mixer().unwrap();
    control.seek(seconds(2)).unwrap();
    let mut samples = vec![0.0; 960];
    mixer.process(&mut samples);
    assert!(samples.iter().all(|sample| (*sample - 0.25).abs() < 0.01));
    assert_eq!(
        mixer.position_frames(),
        96_480,
        "one frame per output frame at 1x"
    );

    // Seek outside the initial four-second window, and install the replacement
    // into the already running callback rather than rebuilding a toy mixer.
    await_audio(&mut audio, seconds(8));
    audio.install_ready(&mut control).unwrap();
    control.seek(seconds(8)).unwrap();
    mixer.process(&mut samples);
    assert!(samples.iter().all(|sample| (*sample + 0.25).abs() < 0.01));

    let mut trimmed = project.clone();
    let clip = trimmed.sequences[0].tracks[0]
        .items
        .iter_mut()
        .find_map(|item| match item {
            sub_model::TrackItem::Clip(clip) => Some(clip),
            _ => None,
        })
        .unwrap();
    clip.source_range = TimeRange::new(seconds(3), seconds(2)).unwrap();
    audio
        .configure(&trimmed, &trimmed.sequences[0], wav.0.parent().unwrap())
        .unwrap();
    await_audio(&mut audio, seconds(2));
    let (mut control, mut mixer) = audio.create_mixer().unwrap();
    control.seek(seconds(2)).unwrap();
    mixer.process(&mut samples);
    assert!(
        samples.iter().all(|sample| (*sample + 0.25).abs() < 0.01),
        "source inpoint must survive the drop's timeline gap"
    );
}

#[test]
fn av_source_drop_routes_audible_samples_to_the_device_callback() {
    let Some(path) = sub_test_support::try_fixture("playback_av.mp4") else {
        eprintln!("skipping: missing playback_av.mp4; routine runner generates it");
        return;
    };
    sub_media::init().unwrap();
    let probed = sub_media::probe(&path).unwrap();
    assert!(probed.has_video() && probed.has_audio());
    let mut item = MediaItem::new(MediaPath::external(&path).unwrap());
    item.info = Some(StreamInfo {
        duration: probed.duration,
        video: vec![sub_model::media::VideoStream {
            width: 320,
            height: 180,
            frame_rate: Rational::FPS_24,
            sample_aspect: Rational::ONE,
            color: sub_model::sequence::ColorTags::REC709,
        }],
        audio: vec![AudioStream {
            channels: 2,
            sample_rate: 48_000,
        }],
    });
    let media = item.id;
    let mut project = Project::new("A/V first drop");
    project.media.push(item);
    let sequence = Sequence::new("Main", SequenceSettings::default());
    let plan = plan_timeline_source_edit(
        &project,
        &sequence,
        media,
        0,
        seconds(0),
        EditMode::Overwrite,
    )
    .unwrap();
    apply_source_edit(&mut History::new(), &mut project, plan).unwrap();
    assert!(
        project.sequences[0]
            .tracks
            .iter()
            .any(|track| track.kind == TrackKind::Audio)
    );
    let mut audio = PlaybackAudio::new(48_000).unwrap();
    audio
        .configure(&project, &project.sequences[0], path.parent().unwrap())
        .unwrap();
    await_audio(&mut audio, seconds(0));
    let (_, mut mixer) = audio.create_mixer().unwrap();
    let mut samples = vec![0.0; 9_600];
    mixer.process(&mut samples);
    assert!(
        samples.iter().any(|sample| sample.abs() > 0.01),
        "first A/V drop must sound through the same callback mixer the device opens"
    );
    assert_eq!(mixer.position_frames(), 4_800);
}
