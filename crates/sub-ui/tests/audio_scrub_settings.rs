//! The scrub settings in the audio panel, on the shared `egui_kittest`
//! harness.
//!
//! Scrubbing is a setting, not a hidden constant: the audio settings panel
//! carries the enable toggle and the grain length, and hands the application a
//! [`AudioSettingsAction::SetScrub`] when either changes (docs/PLAN.md §5.4).
//! This drives the checkbox by its accessibility label over a real
//! [`AudioSettingsPanel`] and then applies what comes back to a real
//! [`ScrubControl`], so the click really does reach the player the audio
//! callback reads.

mod support;

use egui_kittest::kittest::Queryable;
use sub_audio::scrub::{MAX_GRAIN_MS, MIN_GRAIN_MS, ScrubSettings, scrub};
use sub_ui::audio_settings::{AudioSettingsAction, AudioSettingsPanel};

/// The sequence sample rate the player counts at.
const SEQUENCE_RATE: u32 = 48_000;

/// The panel and the last thing it asked for.
///
/// `egui_kittest` runs several frames per `run`, and only the frame the click
/// lands on asks for anything, so the state keeps the last action that was not
/// `None` rather than whatever the final frame returned.
struct PanelState {
    panel: AudioSettingsPanel,
    action: AudioSettingsAction,
}

impl PanelState {
    /// A panel showing `settings`, having asked for nothing yet.
    fn new(settings: ScrubSettings) -> Self {
        let mut panel = AudioSettingsPanel::new();
        panel.set_scrub(settings);
        Self {
            panel,
            action: AudioSettingsAction::None,
        }
    }

    /// Paints one frame, keeping anything the user asked for.
    fn frame(&mut self, ui: &mut eframe::egui::Ui) {
        let action = self.panel.ui(ui);
        if action != AudioSettingsAction::None {
            self.action = action;
        }
    }
}

#[test]
fn the_toggle_reaches_the_player_the_callback_reads() {
    let mut harness =
        support::panel_harness_state(PanelState::new(ScrubSettings::default()), |ui, state| {
            state.frame(ui);
        });

    harness.run();
    harness.get_by_label("Play audio while scrubbing").click();
    harness.run();

    let AudioSettingsAction::SetScrub(settings) = harness.state().action.clone() else {
        panic!(
            "clicking the toggle asks for scrub settings, got {:?}",
            harness.state().action
        );
    };
    assert!(!settings.enabled, "the toggle turned scrubbing off");

    // What the panel asked for is what the audio player is told.
    let (control, player) = scrub(ScrubSettings::default(), SEQUENCE_RATE).expect("a player");
    assert!(player.is_enabled());
    control.apply(settings).expect("valid settings");
    assert!(!player.is_enabled(), "the player heard the toggle");
}

#[test]
fn the_grain_length_is_a_setting_within_the_players_range() {
    let (control, player) = scrub(ScrubSettings::default(), SEQUENCE_RATE).expect("a player");
    let mut panel = AudioSettingsPanel::new();
    panel.set_scrub(ScrubSettings::default().with_grain_ms(MAX_GRAIN_MS));
    assert_eq!(panel.scrub().grain_ms(), MAX_GRAIN_MS);

    control
        .apply(panel.scrub())
        .expect("the panel's range is the player's range");
    assert_eq!(player.grain_frames(), 24_000, "500 ms at 48 kHz");

    control
        .apply(ScrubSettings::default().with_grain_ms(MIN_GRAIN_MS))
        .expect("the shortest grain is playable");
    assert_eq!(player.grain_frames(), 240, "5 ms at 48 kHz");

    let error = control
        .apply(ScrubSettings::default().with_grain_ms(MAX_GRAIN_MS + 1))
        .expect_err("longer than the panel offers");
    assert_eq!(error.code, sub_core::codes::INVALID_ARGUMENT);
    assert_eq!(
        player.grain_frames(),
        240,
        "a rejected setting leaves the player as it was"
    );
}

#[test]
fn the_panel_shows_what_the_audio_stage_actually_applied() {
    let mut harness = support::panel_harness_state(
        PanelState::new(ScrubSettings::default().with_grain_ms(120)),
        |ui, state| state.frame(ui),
    );
    harness.run();
    assert_eq!(
        harness.state().action,
        AudioSettingsAction::None,
        "painting the panel asks for nothing on its own"
    );
    assert_eq!(harness.state().panel.scrub().grain_ms(), 120);
}
