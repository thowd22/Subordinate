//! `subordinate-cli schema` prints the Command API JSON Schema on stdout, and
//! what it prints is exactly the copy committed under `docs/schema/`.

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

/// The committed document, which is the contract the MCP bridge reads.
fn committed() -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(sub_command::schema::COMMITTED_PATH);
    let text = std::fs::read_to_string(path).expect("the committed schema is present");
    serde_json::from_str(&text).expect("the committed schema is JSON")
}

#[test]
fn schema_lists_every_method_with_params_and_result_schemas() {
    let (ok, stdout, stderr) = run(&["schema"]);
    assert!(ok, "schema failed: {stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("schema prints JSON");

    assert_eq!(document["title"], sub_command::schema::TITLE);
    assert_eq!(document["$schema"], sub_command::schema::META_SCHEMA);

    let methods = document["methods"]
        .as_array()
        .expect("methods are an array");
    assert!(methods.len() > 30, "only {} methods", methods.len());
    for method in methods {
        let name = method["name"].as_str().expect("a method name");
        assert!(
            matches!(
                method["kind"].as_str(),
                Some("command" | "query" | "session")
            ),
            "{name} has an unknown kind",
        );
        let description = method["description"].as_str().expect("a description");
        assert!(description.ends_with('.'), "{name}: {description:?}");
        assert!(method["params"].is_object(), "{name} has no params schema");
        assert!(method["result"].is_object(), "{name} has no result schema");
    }
    assert!(document["$defs"].is_object(), "the document has no $defs");
}

#[test]
fn what_the_cli_prints_is_what_is_committed() {
    let (ok, stdout, stderr) = run(&["schema"]);
    assert!(ok, "schema failed: {stderr}");
    let printed: serde_json::Value = serde_json::from_str(&stdout).expect("schema prints JSON");
    assert_eq!(
        printed,
        committed(),
        "docs/schema/command-api.json is stale; regenerate it with \
         SUB_UPDATE_SCHEMA=1 cargo test -p sub-command committed_schema_is_up_to_date",
    );
}

#[test]
fn compact_prints_the_same_document_on_one_line() {
    let (ok, stdout, stderr) = run(&["schema", "--compact"]);
    assert!(ok, "schema --compact failed: {stderr}");
    assert_eq!(stdout.lines().count(), 1, "--compact printed several lines");
    let compact: serde_json::Value = serde_json::from_str(&stdout).expect("schema prints JSON");
    assert_eq!(compact, committed());
}
