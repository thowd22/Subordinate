//! The plugin author guide, kept honest (TASK-108).
//!
//! `docs/plugin-guide.md` is what a plugin author — human or agent — works
//! from, so the parts of it that are really interface must be checked against
//! the interface: the worlds a manifest may declare, the capability keys it may
//! ask for, the error codes it promises, and the `plugin.toml` it shows, which
//! has to be a manifest this host would actually accept.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sub_plugin::errors::CATALOGUE;
use sub_plugin::manifest::{Manifest, World};

/// The guide, compiled in so the test cannot be run against another checkout.
const GUIDE: &str = include_str!("../../../docs/plugin-guide.md");

/// The repository root, from this crate's manifest.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the workspace root is two directories up")
        .to_path_buf()
}

/// Everything the guide writes as `code`, outside its fenced blocks.
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

/// The bodies of the guide's fenced blocks of one language.
fn fenced_blocks(language: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in GUIDE.lines() {
        match (&mut current, line.starts_with("```")) {
            (Some(block), false) => {
                block.push_str(line);
                block.push('\n');
            }
            (Some(_), true) => blocks.push(current.take().expect("a block is open")),
            (None, true) if line.trim_end() == format!("```{language}") => {
                current = Some(String::new());
            }
            _ => {}
        }
    }
    assert!(current.is_none(), "the guide has an unclosed fenced block");
    blocks
}

#[test]
fn the_guide_names_every_world_a_manifest_may_declare() {
    let spans = code_spans();
    for world in World::ALL {
        assert!(
            spans.contains(world.as_str()),
            "docs/plugin-guide.md does not mention the `{}` world",
            world.as_str(),
        );
    }
}

#[test]
fn the_guide_names_every_capability_key() {
    let spans = code_spans();
    for key in ["fs_read", "fs_write", "network", "shaders"] {
        assert!(
            spans.contains(key),
            "docs/plugin-guide.md does not mention the `{key}` capability",
        );
    }
    for root in ["$PROJECT", "$PLUGIN_DATA"] {
        assert!(
            GUIDE.contains(root),
            "docs/plugin-guide.md does not mention the {root} root",
        );
    }
}

#[test]
fn every_manifest_the_guide_shows_is_one_this_host_accepts() {
    let blocks = fenced_blocks("toml");
    assert!(!blocks.is_empty(), "the guide shows no plugin.toml");
    for block in blocks {
        let manifest = Manifest::parse(&block)
            .unwrap_or_else(|error| panic!("the guide's manifest does not parse: {error}"));
        assert!(
            !manifest.plugin.worlds.is_empty(),
            "the guide's manifest declares no world",
        );
    }
}

/// The `plugin.*` spans that are manifest paths rather than error codes: the
/// file itself, and the keys of its `[plugin]` table.
const MANIFEST_PATHS: &[&str] = &[
    "plugin.toml",
    "plugin.id",
    "plugin.name",
    "plugin.version",
    "plugin.api",
    "plugin.worlds",
    "plugin.description",
    "plugin.authors",
];

#[test]
fn every_plugin_code_the_guide_names_is_in_the_catalogue() {
    let known: BTreeSet<&str> = CATALOGUE.iter().map(|info| info.code).collect();
    for span in code_spans() {
        // `plugin.*` is how the guide writes the family as a whole.
        if !span.starts_with("plugin.")
            || span.ends_with('*')
            || MANIFEST_PATHS.contains(&span.as_str())
        {
            continue;
        }
        assert!(
            known.contains(span.as_str()),
            "docs/plugin-guide.md names `{span}`, which is not a code this host raises",
        );
    }
}

#[test]
fn the_guide_points_at_the_worked_examples_that_are_there() {
    let root = repository();
    for path in [
        "docs/mcp-guide.md",
        "docs/agent-runbook.md",
        "docs/schema/plugin-manifest.json",
        "wit/subordinate-plugin.wit",
        "wit/README.md",
        "plugins/color",
        "plugins/gain",
        "plugins/cut-silence",
        "plugins/otio",
    ] {
        assert!(
            GUIDE.contains(path),
            "docs/plugin-guide.md does not link {path}",
        );
        assert!(root.join(path).exists(), "{path} is not in the repository");
    }
}
