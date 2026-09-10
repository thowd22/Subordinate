//! A `plugin` failure leaves the CLI as JSON on stderr, on the same terms as
//! a success: a stable code, a message, the WIT item it belongs to when there
//! is one, and a hint (TASK-92, docs/PLAN.md §6.4).

use std::path::PathBuf;
use std::process::Command;

/// Runs the CLI and answers whether it succeeded, its stdout and its stderr.
fn run(args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_subordinate-cli"))
        .args(args)
        .env("SUBORDINATE_LOG", "error")
        .env_remove("RUST_LOG")
        .output()
        .expect("subordinate-cli runs");
    (
        output.status.success(),
        String::from_utf8(output.stdout).expect("utf-8 stdout"),
        String::from_utf8(output.stderr).expect("utf-8 stderr"),
    )
}

/// The last line of stderr, which is the JSON error: `--compact` puts the
/// whole object on one line, and tracing writes nothing at `error` level here.
fn error_json(stderr: &str) -> serde_json::Value {
    let line = stderr
        .lines()
        .last()
        .unwrap_or_else(|| panic!("stderr is empty"));
    serde_json::from_str(line).unwrap_or_else(|err| panic!("stderr is not JSON: {err}: {stderr}"))
}

/// A scratch plugin directory that holds nothing.
fn empty_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("subordinate-cli-plugin-errors-{name}"));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

#[test]
fn a_missing_plugin_is_reported_as_json_with_a_code_and_a_hint() {
    let dir = empty_dir("missing");
    let (ok, stdout, stderr) = run(&[
        "plugin",
        "enable",
        "com.example.absent",
        "--dir",
        &dir.display().to_string(),
        "--compact",
    ]);

    assert!(!ok, "the plugin is not installed, so this must fail");
    assert!(stdout.is_empty(), "stdout: {stdout:?}");
    let error = error_json(&stderr);
    assert_eq!(error["code"], "plugin.not_installed");
    assert!(error["message"].is_string(), "{error}");
    let hint = error["details"]["hint"]
        .as_str()
        .unwrap_or_else(|| panic!("no hint in {error}"));
    assert!(hint.contains("plugin list"), "{hint}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_error_is_indented_by_default_and_one_line_with_compact() {
    let dir = empty_dir("shape");
    let args = |compact: bool| {
        let mut args = vec![
            "plugin".to_owned(),
            "remove".to_owned(),
            "com.example.absent".to_owned(),
            "--dir".to_owned(),
            dir.display().to_string(),
        ];
        if compact {
            args.push("--compact".to_owned());
        }
        args
    };

    let owned = args(false);
    let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
    let (_, _, pretty) = run(&borrowed);
    assert!(
        pretty.contains("\n  \"code\": \"plugin.not_installed\""),
        "the default is indented JSON: {pretty:?}"
    );

    let owned = args(true);
    let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
    let (_, _, compact) = run(&borrowed);
    assert_eq!(compact.lines().count(), 1, "compact: {compact:?}");
    assert_eq!(error_json(&compact)["code"], "plugin.not_installed");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_malformed_id_names_the_offending_id_beside_its_hint() {
    let dir = empty_dir("bad-id");
    let (ok, _, stderr) = run(&[
        "plugin",
        "disable",
        "nodots",
        "--dir",
        &dir.display().to_string(),
        "--compact",
    ]);

    assert!(!ok);
    let error = error_json(&stderr);
    assert_eq!(error["code"], "plugin.invalid_plugin_id");
    assert_eq!(error["details"]["id"], "nodots");
    assert!(
        error["details"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("dot-separated")),
        "{error}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn installing_something_that_is_not_a_plugin_says_what_to_point_at() {
    let dir = empty_dir("install");
    let (ok, _, stderr) = run(&[
        "plugin",
        "install",
        &dir.join("nothing.wasm").display().to_string(),
        "--dir",
        &dir.display().to_string(),
        "--compact",
    ]);

    assert!(!ok);
    let error = error_json(&stderr);
    assert_eq!(error["code"], "plugin.dev_source_invalid");
    assert!(
        error["details"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("plugin.toml")),
        "{error}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
