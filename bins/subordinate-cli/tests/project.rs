//! `subordinate-cli new`, `open`, `save` and `inspect` on a sample project.
//!
//! The round trip is the point: the CLI writes a project, reads it back,
//! reports its structure and rewrites it byte for byte. That is the headless
//! smoke test CI runs and the fallback the MCP bridge relies on.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// Runs the CLI and returns success, stdout and stderr.
fn run(args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_subordinate-cli"))
        .args(args)
        .env("SUBORDINATE_LOG", "warn")
        .env_remove("RUST_LOG")
        .output()
        .expect("subordinate-cli runs");
    (
        output.status.success(),
        String::from_utf8(output.stdout).expect("utf-8 stdout"),
        String::from_utf8(output.stderr).expect("utf-8 stderr"),
    )
}

/// Runs the CLI and parses the JSON it printed.
fn json(args: &[&str]) -> Value {
    let (ok, stdout, stderr) = run(args);
    assert!(ok, "{args:?} failed: {stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|err| panic!("{args:?} printed no JSON: {err}"))
}

/// A directory of this test's own, removed and recreated on each run.
fn workspace(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("sub-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a test directory");
    directory
}

/// The path as the CLI takes it.
fn arg(path: &Path) -> String {
    path.display().to_string()
}

#[test]
fn a_new_project_is_created_opened_inspected_and_saved_again() {
    let directory = workspace("round-trip");
    let path = directory.join("Doc cut.sub");

    let created = json(&["new", &arg(&path), "--compact"]);
    assert_eq!(created["project"]["name"], "Doc cut");
    assert_eq!(created["schema_version"], 1);
    let sequences = created["sequences"].as_array().expect("a sequence list");
    assert_eq!(sequences.len(), 1, "one starter sequence: {created}");
    assert_eq!(sequences[0]["name"], "Main");
    assert_eq!(sequences[0]["tracks"][0], "V1");
    assert_eq!(sequences[0]["tracks"][1], "A1");
    assert!(path.exists(), "the file was not written");

    // A project file is deterministic text, so the file itself is diffable.
    let text = std::fs::read_to_string(&path).expect("the project reads back");
    assert!(text.starts_with("{\n  \"project\": {\n"), "text: {text}");
    assert!(text.ends_with("\"schema_version\": 1\n}\n"), "text: {text}");

    let opened = json(&["open", &arg(&path), "--compact"]);
    assert_eq!(opened["project"], created["project"]);
    assert_eq!(opened["schema_version"]["file"], 1);
    assert_eq!(opened["schema_version"]["migrated"], false);
    assert_eq!(opened["counts"]["sequences"], 1);
    assert_eq!(opened["counts"]["media"], 0);
    assert!(
        opened["offline"]
            .as_array()
            .expect("an offline list")
            .is_empty(),
        "a project with no media has nothing offline: {opened}"
    );

    let inspected = json(&["inspect", &arg(&path), "--compact"]);
    assert_eq!(inspected["project"], created["project"]);
    let sequence = &inspected["sequences"][0];
    assert_eq!(sequence["settings"]["resolution"]["width"], 1920);
    assert_eq!(sequence["settings"]["frame_rate"]["numerator"], 24);
    assert_eq!(sequence["settings"]["frame_rate"]["denominator"], 1);
    assert_eq!(sequence["settings"]["sample_rate"], 48_000);
    // Times are exact rational values, never floats.
    assert_eq!(sequence["duration"]["value"], 0);
    assert_eq!(sequence["duration"]["rate"]["numerator"], 24);
    assert_eq!(sequence["duration"]["timecode"], "00:00:00:00");
    let tracks = sequence["tracks"].as_array().expect("a track list");
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0]["kind"], "video");
    assert_eq!(tracks[1]["kind"], "audio");
    assert!(
        tracks[0]["items"]
            .as_array()
            .expect("an item list")
            .is_empty()
    );
    assert_eq!(
        inspected["bins"]["children"].as_array().map(Vec::len),
        Some(0)
    );

    // Saving over the file changes nothing, because the bytes are a pure
    // function of the model.
    let saved = json(&["save", &arg(&path), "--compact"]);
    assert_eq!(saved["changed"], false);
    assert_eq!(saved["migrated"], false);
    assert_eq!(
        std::fs::read_to_string(&path).expect("still readable"),
        text
    );

    // Saving elsewhere copies it byte for byte.
    let copy = directory.join("copy.sub");
    let saved = json(&["save", &arg(&path), "--output", &arg(&copy), "--compact"]);
    assert_eq!(saved["output"], arg(&copy));
    assert_eq!(saved["changed"], true);
    assert_eq!(
        std::fs::read_to_string(&copy).expect("the copy reads"),
        text
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn new_names_the_project_after_the_file_and_refuses_to_overwrite() {
    let directory = workspace("overwrite");
    let path = directory.join("first.sub");

    let created = json(&["new", &arg(&path), "--compact"]);
    assert_eq!(created["project"]["name"], "first");

    let (ok, _, stderr) = run(&["new", &arg(&path), "--compact"]);
    assert!(!ok, "the second new should have failed");
    let error: Value = serde_json::from_str(&stderr).expect("a JSON SubError on stderr");
    assert_eq!(error["code"], "core.invalid_argument");
    assert_eq!(error["details"]["path"], arg(&path));

    let replaced = json(&[
        "new",
        &arg(&path),
        "--name",
        "Doc cut",
        "--force",
        "--compact",
    ]);
    assert_eq!(replaced["project"]["name"], "Doc cut");
    assert_ne!(replaced["project"]["id"], created["project"]["id"]);

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_file_that_is_not_a_project_is_refused_with_its_path() {
    let directory = workspace("bad-file");
    let path = directory.join("not-a-project.sub");
    std::fs::write(&path, "{\"schema_version\": 999, \"project\": {}}\n").expect("write");

    for subcommand in ["open", "inspect", "save"] {
        let (ok, _, stderr) = run(&[subcommand, &arg(&path), "--compact"]);
        assert!(!ok, "{subcommand} accepted a file it cannot read");
        let error: Value = serde_json::from_str(&stderr).expect("a JSON SubError on stderr");
        assert_eq!(
            error["code"], "model.unsupported_schema_version",
            "{subcommand}: {error}"
        );
        assert_eq!(error["details"]["path"], arg(&path));
    }

    let missing = directory.join("absent.sub");
    let (ok, _, stderr) = run(&["open", &arg(&missing), "--compact"]);
    assert!(!ok, "open accepted a missing file");
    let error: Value = serde_json::from_str(&stderr).expect("a JSON SubError on stderr");
    assert_eq!(error["code"], "core.io");

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_subcommand_without_a_path_prints_the_usage_text() {
    let (ok, stdout, stderr) = run(&["inspect"]);
    assert!(!ok, "inspect with no path should fail");
    assert!(stdout.is_empty(), "nothing belongs on stdout: {stdout}");
    assert!(stderr.contains("Usage:"), "stderr: {stderr}");
}
