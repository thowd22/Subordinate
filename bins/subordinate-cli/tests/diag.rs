//! `subordinate-cli diag` reports the hardware diagnostics as JSON on stdout.

use std::process::Command;

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

#[test]
fn diag_prints_a_parseable_report_of_every_vendor() {
    let (ok, stdout, stderr) = run(&["diag"]);
    assert!(ok, "diag failed: {stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("diag prints JSON");
    assert!(
        report["gstreamer_version"]
            .as_str()
            .expect("a version string")
            .starts_with("1."),
        "report: {report}"
    );
    assert_eq!(report["platform"], std::env::consts::OS);
    let vendors = report["vendors"].as_array().expect("vendors are an array");
    let names: Vec<&str> = vendors
        .iter()
        .map(|vendor| vendor["vendor"].as_str().expect("a vendor id"))
        .collect();
    assert_eq!(names, ["nvcodec", "va", "amf", "vtenc", "mf", "x264"]);
    for vendor in vendors {
        let elements = vendor["elements"]
            .as_array()
            .expect("elements are an array");
        assert!(!elements.is_empty(), "{vendor} lists no elements");
        for element in elements {
            assert!(element["name"].is_string());
            assert!(matches!(
                element["kind"].as_str(),
                Some("decoder" | "encoder")
            ));
            assert!(element["present"].is_boolean());
        }
    }
}

#[test]
fn a_missing_expected_vendor_carries_an_install_hint() {
    let (ok, stdout, _) = run(&["diag", "--compact"]);
    assert!(ok, "diag failed");
    assert_eq!(stdout.lines().count(), 1, "--compact prints one line");
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("diag prints JSON");
    for vendor in report["vendors"].as_array().expect("vendors") {
        let missing = vendor["elements"]
            .as_array()
            .expect("elements")
            .iter()
            .any(|element| element["present"] == false);
        let expected = vendor["expected"] == true;
        if expected && missing {
            let hint = vendor["hint"].as_str().expect("an install hint");
            assert!(
                hint.contains("docs/DEVELOPMENT.md"),
                "hint must cite the install guide: {hint}"
            );
        } else {
            assert!(vendor["hint"].is_null(), "unexpected hint: {vendor}");
        }
    }
}

#[test]
fn an_unknown_argument_fails_with_the_usage_text() {
    let (ok, _, stderr) = run(&["diag", "--nonsense"]);
    assert!(!ok, "an unknown argument must fail");
    assert!(stderr.contains("unknown argument"), "stderr: {stderr}");
    assert!(stderr.contains("subordinate-cli diag"), "stderr: {stderr}");
}
