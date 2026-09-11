//! The MCP guide, kept honest (TASK-108).
//!
//! `docs/mcp-guide.md` is what an agent — or the person configuring one — reads
//! before driving the editor. It is only worth reading if it is true of the
//! build it ships with, and the part of it that can go stale silently is the
//! tool surface: a family the bridge grew that the guide never mentions, a tool
//! the guide names that no longer exists, an environment variable nothing
//! reads. These tests fail instead.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sub_core::logging::FILTER_ENV;
use subordinate_mcp::backend::{CLI_ENV, DIRECTORY_ENV, INSTANCE_ENV, NO_LAUNCH_ENV};
use subordinate_mcp::tools::ToolSet;

/// The guide, compiled in so the test cannot be run against another checkout.
const GUIDE: &str = include_str!("../../../docs/mcp-guide.md");

/// The repository root, from this crate's manifest.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the workspace root is two directories up")
        .to_path_buf()
}

/// Everything the guide writes as `code`, outside its fenced blocks.
///
/// The fences are dropped first: a block of JSON or shell is an example, not a
/// claim about a name, and its own backticks would misalign the spans anyway.
fn code_spans() -> BTreeSet<String> {
    let mut prose = String::new();
    let mut fenced = false;
    for line in GUIDE.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
        } else if !fenced {
            prose.push_str(line);
            prose.push('\n');
        }
    }
    assert!(!fenced, "the guide has an unclosed fenced block");
    prose
        .split('`')
        .skip(1)
        .step_by(2)
        .map(ToOwned::to_owned)
        .collect()
}

/// The tools this build of the bridge serves.
fn tools() -> BTreeSet<String> {
    ToolSet::committed()
        .expect("the compiled-in schemas are valid")
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

#[test]
fn the_guide_names_every_tool_the_bridge_serves() {
    let spans = code_spans();
    let missing: Vec<_> = tools()
        .into_iter()
        .filter(|tool| !spans.contains(tool))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/mcp-guide.md does not mention {missing:?}",
    );
}

#[test]
fn the_guide_names_no_tool_that_does_not_exist() {
    let tools = tools();
    let families: BTreeSet<&str> = tools
        .iter()
        .filter_map(|tool| tool.split_once('_'))
        .map(|(family, _)| family)
        .collect();

    for span in code_spans() {
        let Some((family, _)) = span.split_once('_') else {
            continue;
        };
        if !families.contains(family) {
            // Not a Command API name at all: a plugin-local tool name, a
            // variable, a field.
            continue;
        }
        assert!(
            tools.contains(&span),
            "docs/mcp-guide.md names `{span}`, which this bridge does not serve",
        );
    }
}

#[test]
fn the_guide_documents_the_variables_the_bridge_reads() {
    const KNOWN: [&str; 5] = [
        INSTANCE_ENV,
        DIRECTORY_ENV,
        CLI_ENV,
        NO_LAUNCH_ENV,
        FILTER_ENV,
    ];

    let spans = code_spans();
    for name in KNOWN {
        assert!(
            spans.contains(name),
            "docs/mcp-guide.md does not document {name}",
        );
    }
    for span in &spans {
        if span.starts_with("SUBORDINATE_") {
            assert!(
                KNOWN.contains(&span.as_str()),
                "docs/mcp-guide.md documents {span}, which nothing reads",
            );
        }
    }
}

#[test]
fn the_guide_points_at_the_worked_examples_that_are_there() {
    let root = repository();
    for path in [
        "docs/plugin-guide.md",
        "docs/agent-runbook.md",
        "docs/examples/mcp.json",
        "docs/schema/command-api.json",
        "docs/schema/plugin-api.json",
        "plugins/color",
        "plugins/gain",
        "plugins/cut-silence",
        "plugins/otio",
    ] {
        assert!(
            GUIDE.contains(path),
            "docs/mcp-guide.md does not link {path}",
        );
        assert!(root.join(path).exists(), "{path} is not in the repository");
    }
}
