//! Software CPU latency must never be interpreted as a failed hardware session.
use super::*;
use crate::encoder::{ElementProbe, EncodeRefusal};
use std::cell::Cell;

fn available(names: &[&str]) -> EncoderProbe {
    EncoderProbe::from_probe("test", &|name| {
        if names.contains(&name) {
            ElementProbe::ready()
        } else {
            ElementProbe::missing()
        }
    })
}

fn uhd() -> ExportSettings {
    ExportSettings::new(3840, 2160, Rational::FPS_60, Container::Mkv)
        .with_video_codec(VideoCodec::Av1)
        .with_audio_codec(None)
}

#[test]
fn pinned_software_never_runs_the_five_second_canvas_probe() {
    let probe = available(&["rav1enc"]);
    let mut preferences = EncoderPreferences::new();
    preferences
        .set_override(VideoCodec::Av1, "rav1enc")
        .unwrap();
    let chosen = probe.select(VideoCodec::Av1, &preferences).unwrap();
    let verified = verify_at_canvas_with(&probe, &uhd(), &preferences, chosen, &|_, _, _| {
        panic!("software must reach the actual export rather than a timed dummy frame")
    })
    .unwrap();
    assert_eq!(verified.element, "rav1enc");
}

#[test]
fn failed_hardware_falls_back_without_timing_software_encoding() {
    let probe = available(&["nvav1enc", "rav1enc"]);
    let preferences = EncoderPreferences::new();
    let chosen = probe.select(VideoCodec::Av1, &preferences).unwrap();
    let calls = Cell::new(0);
    let verified = verify_at_canvas_with(
        &probe,
        &uhd(),
        &preferences,
        chosen,
        &|name, width, height| {
            calls.set(calls.get() + 1);
            assert_eq!(
                name, "nvav1enc",
                "only hardware needs another encode session"
            );
            assert_eq!((width, height), (3840, 2160));
            Err(EncodeRefusal::Refused("device session failed".to_owned()))
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(verified.element, "rav1enc");
}

#[test]
fn pinned_hardware_still_reports_its_canvas_failure() {
    let probe = available(&["nvav1enc", "rav1enc"]);
    let mut preferences = EncoderPreferences::new();
    preferences
        .set_override(VideoCodec::Av1, "nvav1enc")
        .unwrap();
    let chosen = probe.select(VideoCodec::Av1, &preferences).unwrap();
    let error = verify_at_canvas_with(&probe, &uhd(), &preferences, chosen, &|_, _, _| {
        Err(EncodeRefusal::Refused("device session failed".to_owned()))
    })
    .unwrap_err();
    assert_eq!(error.code, codes::ENCODER_UNAVAILABLE);
    assert_eq!(error.details["element"], "nvav1enc");
    assert_eq!(error.details["reason"], "device session failed");
}
