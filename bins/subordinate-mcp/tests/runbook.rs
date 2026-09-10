//! The phase 6 exit runbook, kept honest (TASK-102).
//!
//! `docs/agent-runbook.md` holds the prompt a fresh Claude Code session is
//! given, and `scripts/agent-runbook.sh` reads that same block out of the
//! document and sends it. The runbook is therefore code as much as prose: a
//! tool it names that this bridge does not serve, or a fixture that stops
//! having gaps in it, is a run that fails for a reason that has nothing to do
//! with the agent. These tests fail first instead.
//!
//! Nothing here runs an agent. The run itself needs a `claude` CLI, a network
//! and several minutes, which is `scripts/agent-runbook.sh`'s business; what a
//! `cargo test` can check is that the prompt, the tool surface, the script and
//! the fixture still agree with each other.

use std::collections::BTreeSet;

use subordinate_mcp::tools::ToolSet;

/// The runbook, compiled in so the test cannot be run against some other
/// checkout's copy.
const RUNBOOK: &str = include_str!("../../../docs/agent-runbook.md");

/// The driver script.
const SCRIPT: &str = include_str!("../../../scripts/agent-runbook.sh");

/// The fixture project the run is verified against.
const FIXTURE: &str = include_str!("../../../docs/examples/gaps.sub");

/// The plugin the runbook asks for.
const PLUGIN_ID: &str = "com.example.remove-gaps";

/// The prompt block, between the markers the script reads it between.
fn prompt() -> String {
    let (_, rest) = RUNBOOK
        .split_once("<!-- prompt:start -->")
        .expect("the runbook has a prompt block");
    let (block, _) = rest
        .split_once("<!-- prompt:end -->")
        .expect("the prompt block is closed");
    block
        .lines()
        .filter(|line| !line.starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `` `plugin_…` `` name the prompt marks as a tool.
fn tool_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for piece in text.split('`').skip(1).step_by(2) {
        if piece.starts_with("plugin_") && piece.chars().all(|c| c.is_ascii_lowercase() || c == '_')
        {
            names.insert(piece.to_owned());
        }
    }
    names
}

#[test]
fn the_prompt_names_only_tools_the_bridge_serves() {
    let tools = ToolSet::committed().expect("the compiled-in schemas are tools");
    let named = tool_names(&prompt());
    assert!(
        named.contains("plugin_new")
            && named.contains("plugin_install")
            && named.contains("plugin_test"),
        "the prompt has to walk the whole loop: {named:?}",
    );
    for name in &named {
        assert!(
            tools.method(name).is_some(),
            "the runbook names {name}, which this bridge does not serve",
        );
    }
}

#[test]
fn the_prompt_carries_the_placeholders_the_script_renders() {
    let prompt = prompt();
    for placeholder in ["{{WORKDIR}}", "{{SDK}}", "{{FIXTURE}}"] {
        assert!(
            prompt.contains(placeholder),
            "the prompt has no {placeholder} for the script to fill in",
        );
    }
    assert!(
        prompt.contains(PLUGIN_ID),
        "the prompt has to name the plugin id the verdict checks",
    );
}

#[test]
fn the_script_verifies_the_plugin_the_prompt_asks_for() {
    for expected in [
        PLUGIN_ID,
        "docs/agent-runbook.md",
        "docs/examples/gaps.sub",
        "prompt:start",
    ] {
        assert!(
            SCRIPT.contains(expected),
            "the driver script no longer mentions {expected}",
        );
    }
}

#[test]
fn the_fixture_is_three_clips_with_fifteen_frames_of_gap_between_them() {
    let project = sub_model::json::from_json(FIXTURE).expect("the fixture loads");
    let sequence = project
        .sequences
        .first()
        .expect("the fixture has a sequence");
    let rate = sequence.settings.frame_rate;
    let track = sequence.tracks.first().expect("the fixture has a track");

    let clips: Vec<_> = track
        .items
        .iter()
        .filter_map(sub_model::TrackItem::as_clip)
        .collect();
    assert_eq!(
        clips.len(),
        3,
        "three clips, so two gaps have somewhere to be"
    );

    let gaps: Vec<_> = track
        .items
        .iter()
        .filter_map(sub_model::TrackItem::as_gap)
        .collect();
    assert_eq!(gaps.len(), 2, "the fixture is only useful with gaps in it");
    let frames: i64 = gaps
        .iter()
        .map(|gap| gap.duration.rescaled_to(rate).value())
        .sum();
    assert_eq!(frames, 15, "the prompt tells the agent 15 frames come out");
}
