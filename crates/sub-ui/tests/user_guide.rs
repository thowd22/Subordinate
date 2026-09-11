//! The user guide, kept honest (TASK-107).
//!
//! `docs/user-guide.md` is what someone reads before using the editor, so the
//! parts of it that can go stale silently are checked here rather than by
//! eye: the keyboard reference is generated from the action registry and must
//! match it exactly, every screenshot it shows must be a snapshot the UI tests
//! still record, and the troubleshooting table must name every encoder the
//! exporter would actually try.
//!
//! Regenerate the keyboard block after adding or rebinding an action:
//!
//! ```bash
//! UPDATE_DOCS=1 cargo test -p sub-ui --test user_guide
//! ```

use std::path::{Path, PathBuf};

use sub_export::encoder::{CODECS, encoder_names};
use sub_ui::shortcuts::{Action, Category, REFERENCE_BEGIN, REFERENCE_END, ShortcutMap};
use sub_ui::shortcuts::{portable_chord_label, reference_markdown};

/// The environment variable that rewrites the generated block in place.
const UPDATE_ENV: &str = "UPDATE_DOCS";

/// Whether this run is rewriting the guide rather than checking it.
///
/// A rewriting run is the one case where the file changes underneath the
/// tests, so every test that only reads it stands aside: what they would be
/// reading is a document being replaced.
fn updating() -> bool {
    std::env::var(UPDATE_ENV).is_ok_and(|value| !value.is_empty())
}

/// The guide's path, from this crate's manifest.
fn guide_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the workspace root is two directories up")
        .join("docs/user-guide.md")
}

/// The guide as it is committed.
fn guide() -> String {
    std::fs::read_to_string(guide_path()).expect("the user guide is present")
}

/// The text between the two markers, without the marker lines themselves.
fn block(text: &str) -> String {
    let (_, rest) = text
        .split_once(REFERENCE_BEGIN)
        .unwrap_or_else(|| panic!("the guide has no {REFERENCE_BEGIN} marker"));
    let (block, _) = rest
        .split_once(REFERENCE_END)
        .unwrap_or_else(|| panic!("the guide has no {REFERENCE_END} marker"));
    block.trim_matches('\n').to_owned()
}

/// The guide with the generated block replaced by `generated`.
fn with_block(text: &str, generated: &str) -> String {
    let (head, rest) = text.split_once(REFERENCE_BEGIN).expect("a begin marker");
    let (_, tail) = rest.split_once(REFERENCE_END).expect("an end marker");
    format!("{head}{REFERENCE_BEGIN}\n\n{generated}\n{REFERENCE_END}{tail}")
}

/// Every image path the guide shows, resolved against `docs/`.
fn screenshots(text: &str) -> Vec<(String, PathBuf)> {
    let docs = guide_path().parent().expect("docs/").to_path_buf();
    text.match_indices("![")
        .filter_map(|(start, _)| {
            let rest = &text[start..];
            let open = rest.find("](")? + 2;
            let close = rest[open..].find(')')? + open;
            let link = &rest[open..close];
            Some((link.to_owned(), docs.join(link)))
        })
        .collect()
}

#[test]
fn the_keyboard_reference_is_the_action_registry() {
    let generated = reference_markdown(&ShortcutMap::default_map());
    let guide = guide();
    if updating() {
        let updated = with_block(&guide, generated.trim_end());
        let path = guide_path();
        let temporary = path.with_extension("md.new");
        std::fs::write(&temporary, updated).expect("the guide's directory is writable");
        std::fs::rename(&temporary, &path).expect("the guide is replaceable");
        return;
    }
    assert_eq!(
        block(&guide),
        generated.trim_end(),
        "the guide's keyboard reference is stale; rerun with {UPDATE_ENV}=1",
    );
}

#[test]
fn every_action_is_documented_with_its_id_and_chord() {
    if updating() {
        return;
    }

    let map = ShortcutMap::default_map();
    let block = block(&guide());
    for action in Action::ALL {
        assert!(
            block.contains(&format!("`{}`", action.id())),
            "{} is missing from the guide",
            action.id()
        );
        assert!(
            block.contains(action.label()),
            "{} has no label in the guide",
            action.id()
        );
        let chord = map.chord_for(action).expect("the shipped map binds it");
        assert!(
            block.contains(&format!("`{}`", portable_chord_label(chord))),
            "{} has no chord in the guide",
            action.id()
        );
    }
    for category in Category::ALL {
        assert!(
            block.contains(&format!("### {}", category.label())),
            "the guide has no {} section",
            category.label()
        );
    }
}

#[test]
fn every_screenshot_is_a_recorded_snapshot() {
    if updating() {
        return;
    }

    let guide = guide();
    let shots = screenshots(&guide);
    assert!(shots.len() >= 10, "only {} screenshots", shots.len());
    for (link, path) in shots {
        assert!(
            link.starts_with("../crates/sub-ui/tests/snapshots/"),
            "{link} is not a committed UI snapshot",
        );
        assert!(path.is_file(), "{link} does not exist");
    }
}

#[test]
fn every_mvp_feature_has_a_section() {
    if updating() {
        return;
    }

    let guide = guide();
    for heading in [
        "## Importing media",
        "## Editing",
        "## Audio",
        "## Export",
        "## Proxies",
        "## The pop-out viewer",
        "## Keyboard shortcuts",
        "## Troubleshooting",
    ] {
        assert!(
            guide.contains(heading),
            "the guide has no {heading} section"
        );
    }
}

#[test]
fn the_troubleshooting_section_names_every_encoder() {
    if updating() {
        return;
    }

    let guide = guide();
    let (_, troubleshooting) = guide
        .split_once("## Troubleshooting")
        .expect("a troubleshooting section");
    for codec in CODECS {
        for element in encoder_names(codec) {
            assert!(
                troubleshooting.contains(element),
                "{element} is not in the troubleshooting section",
            );
        }
    }
}
