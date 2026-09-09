//! The committed `.mcp.json` example.
//!
//! `docs/examples/mcp.json` is what a user copies into a project so Claude Code
//! starts this bridge (docs/PLAN.md §7). It is only useful if it stays true, so
//! it is checked here against the binary's own names: the executable it points
//! at, and the environment variables the bridge actually reads.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sub_core::logging::FILTER_ENV;
use subordinate_mcp::backend::{CLI_ENV, DIRECTORY_ENV, INSTANCE_ENV, NO_LAUNCH_ENV};

/// The repository root, from this crate's manifest.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the workspace root is two directories up")
        .to_path_buf()
}

/// The committed example, parsed.
fn example() -> Value {
    let path = repository().join("docs/examples/mcp.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()));
    serde_json::from_str(&text).expect("the example is JSON")
}

#[test]
fn the_example_registers_this_bridge_over_stdio() {
    let config = example();
    let server = &config["mcpServers"]["subordinate"];
    assert_eq!(server["type"], "stdio");
    let command = server["command"].as_str().expect("a command");
    assert!(
        command.ends_with(env!("CARGO_PKG_NAME")),
        "{command} does not name this binary",
    );
    assert!(server["args"].as_array().expect("args").is_empty());
}

#[test]
fn the_example_only_sets_variables_the_bridge_reads() {
    let config = example();
    let env = config["mcpServers"]["subordinate"]["env"]
        .as_object()
        .expect("an env object");
    let known = [
        INSTANCE_ENV,
        DIRECTORY_ENV,
        CLI_ENV,
        NO_LAUNCH_ENV,
        FILTER_ENV,
    ];
    for name in env.keys() {
        assert!(
            known.contains(&name.as_str()),
            "{name} is not read by anything",
        );
    }
    assert_eq!(env[INSTANCE_ENV], "default");
}
