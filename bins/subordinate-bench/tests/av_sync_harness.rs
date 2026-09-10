//! The A/V sync harness end to end, as CI runs it.
//!
//! The binary is run the way the workflow runs it — `--sync`, a report path,
//! nothing else assumed about the machine — and the JSON it writes is read
//! back. Where the ten-minute fixture has not been generated the run is a
//! skipped section and still exits 0, which is the behaviour that keeps the
//! harness from failing a build over a missing file; where it has, the picture
//! must have stayed inside one frame of the audio clock.
//!
//! Only a few seconds of the fixture are played here. The whole ten minutes is
//! what CI measures (docs/PERFORMANCE.md).

use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// Seconds of the fixture the test plays.
const SECONDS: &str = "5";

/// One whole frame, in the milli-frames the report counts drift in.
const FRAME_MILLI: i64 = 1_000;

#[test]
fn the_sync_harness_runs_and_reports_drift_or_says_why_it_could_not() {
    let out = std::env::temp_dir().join(format!("subordinate-av-sync-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&out);

    let status = Command::new(env!("CARGO_BIN_EXE_subordinate-bench"))
        .args(["--sync", "--seconds", SECONDS, "--out"])
        .arg(&out)
        .status()
        .expect("the harness runs");
    assert!(
        status.success(),
        "the harness exited with {status}; a missing fixture must not fail it"
    );

    let report = read_report(&out);
    let sync = report
        .get("sync")
        .expect("a sync run always writes a sync section");
    match sync["status"].as_str() {
        Some("skipped") => {
            assert!(
                sync["skipped_reason"]
                    .as_str()
                    .is_some_and(|r| !r.is_empty()),
                "a skipped run must say why: {sync}"
            );
        }
        Some("measured") => {
            let drift = sync["max_drift_milli_frames"]
                .as_i64()
                .expect("drift is an integer count of milli-frames");
            assert!(
                drift.abs() < FRAME_MILLI,
                "the picture drifted {drift} milli-frames from the audio clock: {sync}"
            );
            assert!(
                sync["samples"].as_u64().is_some_and(|samples| samples > 0),
                "a measured run samples at least once a second: {sync}"
            );
            assert!(
                sync["frames_shown"].as_u64().is_some_and(|shown| shown > 0),
                "a measured run puts frames on screen: {sync}"
            );
        }
        other => panic!("unexpected sync status {other:?} in {sync}"),
    }
}

/// The report the harness wrote, parsed.
fn read_report(out: &Path) -> Value {
    let json = std::fs::read_to_string(out).expect("the harness wrote its report");
    serde_json::from_str(&json).expect("the report is JSON")
}
