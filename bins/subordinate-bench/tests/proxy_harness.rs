//! The proxy editing validation end to end.
//!
//! The binary is run the way phase 5's exit criterion is checked — `--proxy`,
//! a report path, a scratch proxy cache — and the JSON it writes is read back.
//! Where the one-hour fixture has not been generated (it never is in CI: it
//! costs a quarter of an hour to encode) the run is a skipped section and
//! still exits 0, which is what keeps the harness from failing a build over a
//! missing file. Where it has been generated, the section must carry the three
//! numbers the criterion is stated in, and the proxy must be the intra-only
//! stand-in it claims to be.
//!
//! The run here is deliberately short — a couple of seeks and a second of
//! playback. The full run and its recorded numbers live in
//! docs/PERFORMANCE.md.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

#[test]
fn the_proxy_harness_runs_and_reports_the_criteria_or_says_why_it_could_not() {
    let out = std::env::temp_dir().join(format!("subordinate-proxy-{}.json", std::process::id()));
    // The cache path deliberately carries no process id: where the fixture
    // does exist, the first run transcodes an hour of video and every run
    // after it reuses that proxy instead of paying for it again.
    let cache = std::env::temp_dir().join("subordinate-proxy-cache");
    let _ = std::fs::remove_file(&out);

    let output = Command::new(env!("CARGO_BIN_EXE_subordinate-bench"))
        .args(["--proxy", "--seconds", "1", "--seeks", "2", "--out"])
        .arg(&out)
        .arg("--proxy-cache")
        .arg(&cache)
        .output()
        .expect("the harness runs");

    let report = read_report(&out);
    let proxy = report
        .get("proxy")
        .expect("a proxy run always writes a proxy section");
    match proxy["status"].as_str() {
        Some("skipped") => {
            assert!(
                output.status.success(),
                "a missing fixture must not fail the harness: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                proxy["skipped_reason"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty()),
                "a skipped run must say why: {proxy}"
            );
        }
        Some("measured") => {
            // The three numbers the criterion is stated in are always present.
            assert_eq!(
                proxy["scrub_target_milli_fps"].as_u64(),
                Some(30_000),
                "the scrub target is 30 fps: {proxy}"
            );
            assert!(
                proxy["proxy_scrub_milli_fps"].as_u64().is_some(),
                "a measured run scrubs the proxy: {proxy}"
            );
            assert!(
                proxy["proxy_playback_stalls"].as_u64().is_some(),
                "a measured run counts stalls: {proxy}"
            );
            assert!(
                proxy["memory_budget_bytes"].as_u64().is_some_and(|b| b > 0),
                "a measured run judges memory against a budget: {proxy}"
            );
            // The proxy really is a smaller stand-in for the source.
            let (width, height) = (
                proxy["proxy_width"].as_u64().unwrap_or(0),
                proxy["proxy_height"].as_u64().unwrap_or(0),
            );
            assert_eq!((width, height), (960, 540), "half of 1080p: {proxy}");
            assert!(
                proxy["proxy_frames"]
                    .as_u64()
                    .is_some_and(|frames| frames > 0),
                "the proxy holds frames: {proxy}"
            );
            // Both files were measured, twice each.
            let scenarios = report["scenarios"]
                .as_array()
                .expect("the run records its scenarios");
            assert_eq!(scenarios.len(), 4, "source and proxy, playback and scrub");
        }
        other => panic!("unexpected proxy status {other:?} in {proxy}"),
    }
}

/// The report the harness wrote, parsed.
fn read_report(out: &Path) -> Value {
    let json = std::fs::read_to_string(out).expect("the harness wrote its report");
    serde_json::from_str(&json).expect("the report is JSON")
}
