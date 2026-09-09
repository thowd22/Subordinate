//! The metrics a decode-ahead worker publishes through tracing (TASK-17).
//!
//! Buffer occupancy and per-frame decode time are meant to be readable by
//! anything that installs a subscriber — the app's own logging, a benchmark
//! harness, an agent reading structured logs — so they are asserted the way a
//! reader would see them: through a subscriber that records what actually
//! reaches the `sub_media::decode_ahead` target.
//!
//! The events come off the worker thread, so the subscriber has to be the
//! global one; this file is its own test binary for that reason and holds a
//! single test.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sub_media::{DecodeAhead, DecodeAheadOptions, DecoderOptions, HardwarePreference};
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt as _};

/// The fixture this test reads: 5 s of colour bars at 25 fps.
const FIXTURE: &str = "bars_1080p_h264.mp4";

/// The tracing target the decode-ahead worker reports on.
const TARGET: &str = "sub_media::decode_ahead";

/// Every event seen on the decode-ahead target, rendered as text.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<String>>>);

impl Captured {
    fn events(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("the capture lock is never poisoned")
            .clone()
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Captured {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != TARGET {
            return;
        }
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        if let Ok(mut seen) = self.0.lock() {
            seen.push(format!("{} {}", event.metadata().level(), fields.0));
        }
    }
}

/// Renders an event's fields as `name=value` pairs.
struct Fields(String);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, "{}={value:?} ", field.name());
    }
}

/// The fixture path, or `None` when the fixtures were never generated.
fn fixture() -> Option<PathBuf> {
    let path = sub_test_support::try_fixture(FIXTURE);
    if path.is_none() {
        eprintln!("skipping: no fixture {FIXTURE}; run scripts/gen-fixtures.sh");
    }
    path
}

#[test]
fn occupancy_and_decode_time_are_reported_through_tracing() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::registry().with(captured.clone());
    tracing::subscriber::set_global_default(subscriber)
        .expect("this binary installs the subscriber once");

    let Some(path) = fixture() else { return };

    let mut ahead = DecodeAhead::open_with(
        &path,
        DecodeAheadOptions {
            capacity: 2,
            decoder: DecoderOptions {
                hardware: HardwarePreference::Software,
                frame_timeout: Duration::from_secs(20),
                ..DecoderOptions::default()
            },
        },
    )
    .expect("the fixture opens for decode-ahead");

    for _ in 0..5 {
        ahead
            .next_frame()
            .expect("the fixture decodes ahead")
            .expect("the fixture has frames");
    }
    ahead
        .seek_to(sub_time::RationalTime::new(
            50,
            sub_time::Rational::new(25, 1).expect("25 fps is a valid rate"),
        ))
        .expect("the seek is accepted");
    ahead
        .next_frame()
        .expect("the fixture seeks under decode-ahead")
        .expect("the target frame exists");

    let events = captured.events();
    let frames: Vec<&String> = events
        .iter()
        .filter(|line| line.contains("decode-ahead frame ready"))
        .collect();
    assert!(
        frames.len() >= 5,
        "one event per decoded frame: {events:#?}"
    );
    for event in &frames {
        assert!(
            event.starts_with("TRACE"),
            "per-frame metrics are trace level: {event}"
        );
        for field in ["pts_ns=", "decode_us=", "occupancy=", "capacity="] {
            assert!(event.contains(field), "{field} missing from {event}");
        }
    }
    assert!(
        events
            .iter()
            .any(|line| line.starts_with("DEBUG") && line.contains("decode-ahead seek requested")),
        "a seek is reported with the frames it dropped: {events:#?}"
    );
}
