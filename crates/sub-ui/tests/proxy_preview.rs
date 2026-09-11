//! Proxy state in the bin, and the viewer's preview switch (TASK-70).
//!
//! Two halves, on the shared harness in `tests/support/mod.rs`.
//!
//! The snapshots are one per proxy state: a bin holding a single item in that
//! state, so the committed PNGs show exactly what an editor reads off a row —
//! no proxy, one being generated, one ready, one gone stale under a changed
//! source, and one whose generation failed. A bin painted with nothing else in
//! it keeps each PNG small and the diff on the badge.
//!
//! The interaction half clicks the viewer's `Proxy` button and asserts on
//! which file preview would open afterwards. That is view state — which file
//! the preview reads is not an edit — so it is asserted on the panel and on
//! the model's [`MediaUse`] resolution rather than on a command. The same
//! items resolved for [`MediaUse::Export`] stay on their originals whatever
//! the switch says, which is the half of the criterion a test has to pin: a
//! proxy must never reach the delivered file.

mod support;

use egui_kittest::kittest::Queryable;
use sub_model::media::{StreamInfo, VideoStream};
use sub_model::{ColorTags, MediaItem, MediaPath, MediaUse, Project, ProxyState};
use sub_time::{Rational, RationalTime};
use sub_ui::media_bin::{MediaBinPanel, proxy_text};
use sub_ui::viewer::{PROXY_TOGGLE_LABEL, ViewerPanel};

/// Where the proxy of the fixture item lives, project-relative.
const PROXY_PATH: &str = "cut.sub.d/interview.proxy.mov";

/// Where the source of the fixture item lives, project-relative.
const SOURCE_PATH: &str = "footage/interview.mp4";

/// A 4K item named `interview.mp4` in proxy state `proxy`.
fn item(proxy: ProxyState) -> MediaItem {
    let mut media = MediaItem::new(MediaPath::new(SOURCE_PATH).expect("valid path"));
    media.info = Some(StreamInfo {
        duration: Some(RationalTime::new(240, Rational::FPS_24)),
        video: vec![VideoStream {
            width: 3840,
            height: 2160,
            frame_rate: Rational::FPS_24,
            sample_aspect: Rational::ONE,
            color: ColorTags::REC709,
        }],
        audio: Vec::new(),
    });
    media.proxy = proxy;
    media
}

/// A project whose root bin holds that one item.
fn project_with(proxy: ProxyState) -> Project {
    let mut project = Project::new("Doc cut");
    let media = item(proxy);
    project.root_bin.media.push(media.id);
    project.media.push(media);
    project
}

/// The proxy path as the model stores it.
fn proxy_path() -> MediaPath {
    MediaPath::new(PROXY_PATH).expect("valid path")
}

/// Every state the bin can show, with the name its snapshot is committed under.
fn states() -> Vec<(&'static str, ProxyState)> {
    vec![
        ("proxy_state_none", ProxyState::None),
        ("proxy_state_generating", ProxyState::Pending),
        ("proxy_state_ready", ProxyState::Ready(proxy_path())),
        ("proxy_state_stale", ProxyState::Stale(proxy_path())),
        (
            "proxy_state_failed",
            ProxyState::Failed("no proxy encoder on this machine".to_owned()),
        ),
    ]
}

/// Paints a bin holding one item in `state` and holds it to `name`.
///
/// One harness per test: `egui_kittest` merges the snapshot results of a
/// single harness, so a test that painted all five states at once could not
/// re-record them.
fn snapshot_state(name: &str, state: ProxyState) {
    let project = project_with(state);
    let mut panel = MediaBinPanel::new();
    let mut harness = support::panel_harness(|ui| {
        panel.ui(ui, &project);
    });
    harness.run();
    support::snapshot(&mut harness, name);
}

#[test]
fn a_bin_row_with_no_proxy_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    snapshot_state("proxy_state_none", ProxyState::None);
}

#[test]
fn a_bin_row_with_a_proxy_being_generated_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    snapshot_state("proxy_state_generating", ProxyState::Pending);
}

#[test]
fn a_bin_row_with_a_ready_proxy_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    snapshot_state("proxy_state_ready", ProxyState::Ready(proxy_path()));
}

#[test]
fn a_bin_row_with_a_stale_proxy_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    snapshot_state("proxy_state_stale", ProxyState::Stale(proxy_path()));
}

#[test]
fn a_bin_row_with_a_failed_proxy_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    snapshot_state(
        "proxy_state_failed",
        ProxyState::Failed("no proxy encoder on this machine".to_owned()),
    );
}

#[test]
fn each_proxy_state_names_itself_in_the_bin() {
    for (_, state) in states() {
        let label = state.label();
        let media = item(state);
        let text = proxy_text(&media);
        assert!(
            !text.is_empty(),
            "the {label} state must say something in the bin"
        );
        // The five badges are distinct, so a row is never ambiguous.
        let others: Vec<String> = states()
            .into_iter()
            .filter(|(_, other)| other.label() != label)
            .map(|(_, other)| proxy_text(&item(other)))
            .collect();
        assert!(!others.contains(&text), "{label} shares a badge: {text}");
    }
}

#[test]
fn the_viewer_switch_moves_preview_between_the_proxy_and_the_original() {
    let ready = item(ProxyState::Ready(proxy_path()));
    let panel = ViewerPanel::new(Rational::FPS_24);
    let mut harness = support::panel_harness_state(panel, |ui, panel| {
        panel.ui(ui, None);
    });

    // The switch is on by default, so a ready proxy is what preview opens.
    let source = harness.state().preview_source(&ready);
    assert!(source.is_proxy);
    assert_eq!(source.path.as_str(), PROXY_PATH);

    harness.get_by_label(PROXY_TOGGLE_LABEL).click();
    harness.run();
    assert!(!harness.state().use_proxies);
    let source = harness.state().preview_source(&ready);
    assert!(
        !source.is_proxy,
        "the switch is off: preview reads the original"
    );
    assert_eq!(source.path.as_str(), SOURCE_PATH);

    // And back: the same click returns preview to the proxy.
    harness.get_by_label(PROXY_TOGGLE_LABEL).click();
    harness.run();
    assert!(harness.state().use_proxies);
    assert!(harness.state().preview_source(&ready).is_proxy);
}

#[test]
fn only_a_ready_proxy_is_previewed_from_and_export_never_is() {
    let panel = ViewerPanel::new(Rational::FPS_24);
    assert!(panel.use_proxies, "proxies are on by default");

    for (_, state) in states() {
        let media = item(state.clone());
        let previewed = panel.preview_source(&media).is_proxy;
        assert_eq!(
            previewed,
            state.is_ready(),
            "{} previewed as {previewed}",
            state.label()
        );
        // Export resolves the same item and never lands on the proxy.
        let exported = media.source(MediaUse::Export);
        assert!(
            !exported.is_proxy,
            "export used the {} proxy",
            state.label()
        );
        assert_eq!(exported.path.as_str(), SOURCE_PATH);
    }
}
