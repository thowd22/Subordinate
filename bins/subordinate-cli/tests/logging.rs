//! The binary initialises tracing: env-filter on stderr, with a JSON option.

use std::process::Command;

fn run(format: Option<&str>, filter: &str) -> (String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_subordinate-cli"));
    cmd.env("SUBORDINATE_LOG", filter);
    cmd.env_remove("RUST_LOG");
    match format {
        Some(value) => cmd.env("SUBORDINATE_LOG_FORMAT", value),
        None => cmd.env_remove("SUBORDINATE_LOG_FORMAT"),
    };
    let output = cmd.output().expect("subordinate-cli runs");
    assert!(output.status.success(), "exit status {}", output.status);
    (
        String::from_utf8(output.stdout).expect("utf-8 stdout"),
        String::from_utf8(output.stderr).expect("utf-8 stderr"),
    )
}

#[test]
fn text_logs_go_to_stderr_and_leave_stdout_clean() {
    let (stdout, stderr) = run(None, "info");
    assert!(stdout.starts_with("subordinate-cli "), "stdout: {stdout:?}");
    assert!(!stdout.contains("starting"), "stdout: {stdout:?}");
    assert!(
        stderr.contains("subordinate-cli starting"),
        "stderr: {stderr:?}"
    );
    assert!(stderr.contains("INFO"), "stderr: {stderr:?}");
}

#[test]
fn the_env_filter_suppresses_events_below_the_configured_level() {
    let (_, stderr) = run(None, "warn");
    assert!(stderr.is_empty(), "stderr: {stderr:?}");
}

#[test]
fn json_format_emits_one_json_object_per_event() {
    let (_, stderr) = run(Some("json"), "info");
    let line = stderr.lines().next().expect("one log line");
    let event: serde_json::Value = serde_json::from_str(line).expect("valid JSON event");
    assert_eq!(event["level"], "INFO");
    assert_eq!(event["target"], "subordinate_cli");
    assert_eq!(event["message"], "subordinate-cli starting");
    assert!(event["version"].is_string(), "event: {event}");
}

#[test]
fn an_unusable_log_format_fails_the_process_with_a_sub_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_subordinate-cli"))
        .env("SUBORDINATE_LOG_FORMAT", "yaml")
        .output()
        .expect("subordinate-cli runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
    assert!(
        stderr.contains("[core.invalid_argument]"),
        "stderr: {stderr:?}"
    );
}
