//! The sequence tab strip that sits above the timeline, and the dialog that
//! creates a sequence.
//!
//! A project holds several sequences (docs/PLAN.md §2) and the tab strip is
//! how one of them becomes the one being edited. The strip owns no project
//! state and mutates nothing: clicking a tab, renaming one or deleting one
//! yields a [`SequenceTabAction`], which the host turns into the matching
//! `sub-edit` command so that every mutation stays undoable.
//!
//! What the strip does own is what looking at a sequence means but the model
//! does not store: the zoom, the horizontal scroll, the lane scroll and the
//! playhead. Those are captured per sequence on the way out of a tab and
//! restored on the way back in, so switching sequences is not a reset. Every
//! remembered position is a [`RationalTime`] or an exact zoom fraction; no
//! timeline value is ever a float.

use eframe::egui::{self, Ui};
use sub_core::{SubError, SubResult};
use sub_model::sequence::{Resolution, Sequence, SequenceSettings};
use sub_model::{ColorTags, SequenceId};
use sub_time::{Rational, RationalTime};

use crate::codes;
use crate::timeline::{TimelineView, ZoomLevel};
use crate::timeline_panel::TimelinePanel;
use crate::viewer::{ViewerState, sequence_duration};

/// The timebases the new-sequence dialog offers, in the order it lists them.
pub const FRAME_RATE_PRESETS: [(&str, Rational); 8] = [
    ("23.976", Rational::FPS_23_976),
    ("24", Rational::FPS_24),
    ("25", Rational::FPS_25),
    ("29.97", Rational::FPS_29_97),
    ("30", Rational::FPS_30),
    ("50", Rational::FPS_50),
    ("59.94", Rational::FPS_59_94),
    ("60", Rational::FPS_60),
];

/// The audio sample rates the new-sequence dialog offers.
pub const SAMPLE_RATE_PRESETS: [u32; 3] = [44_100, 48_000, 96_000];

/// The canvas sizes the new-sequence dialog offers.
pub const RESOLUTION_PRESETS: [(&str, Resolution); 2] = [
    ("1920x1080", Resolution::HD_1080),
    ("3840x2160", Resolution::UHD_2160),
];

/// What the user asked the tab strip to do.
///
/// The strip never edits the project itself. Each variant maps onto exactly
/// one command in `sub-edit`: `CreateSequence`, `RenameSequence` and
/// `DeleteSequence`. [`SequenceTabAction::Switch`] is the exception — which
/// sequence is on screen is a view concern, not a project one, so it changes
/// nothing that could be undone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SequenceTabAction {
    /// Show this sequence instead of the active one.
    Switch(SequenceId),
    /// Create a sequence with this name and these settings.
    Create {
        /// Display name for the new sequence.
        name: String,
        /// Canvas, timebase, audio rate and colour tags.
        settings: SequenceSettings,
    },
    /// Rename this sequence.
    Rename {
        /// The sequence to rename.
        sequence: SequenceId,
        /// Its new name.
        name: String,
    },
    /// Delete this sequence and everything on it.
    Delete(SequenceId),
}

/// Where one sequence was last being looked at.
///
/// Captured when a tab is left and restored when it is returned to, which is
/// what makes switching sequences non-destructive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SequenceViewState {
    /// The timeline zoom, in exact pixels per frame.
    pub zoom: ZoomLevel,
    /// Pixels between sequence time zero and the left edge of the lanes.
    pub scroll_px: i64,
    /// How far the lanes were scrolled down, in points.
    lane_scroll_px: f32,
    /// The playhead, at the sequence's own timebase.
    pub playhead: RationalTime,
}

impl SequenceViewState {
    /// The state a sequence is looked at with before it has ever been on
    /// screen: the origin, one pixel per frame, playhead at frame zero.
    #[must_use]
    pub fn initial(sequence: &Sequence) -> Self {
        Self {
            zoom: ZoomLevel::ONE,
            scroll_px: 0,
            lane_scroll_px: 0.0,
            playhead: RationalTime::zero(sequence.settings.frame_rate),
        }
    }

    /// Reads back what the panel and the viewer are currently showing.
    #[must_use]
    pub fn capture(panel: &TimelinePanel, viewer: &ViewerState) -> Self {
        Self {
            zoom: panel.view().zoom(),
            scroll_px: panel.view().scroll_px(),
            lane_scroll_px: panel.lane_scroll_px(),
            playhead: viewer.playhead(),
        }
    }

    /// How far the lanes were scrolled down, in points; never negative.
    #[must_use]
    pub const fn lane_scroll_px(&self) -> f32 {
        self.lane_scroll_px
    }

    /// Puts the panel and the viewer back where this state left them, at
    /// `sequence`'s timebase and length.
    ///
    /// The viewport width is left as the panel finds it: egui hands the panel
    /// its width every frame, so restoring a stale one would only be
    /// overwritten. The playhead is rescaled to the sequence's timebase and
    /// clamped into its duration by [`ViewerState`], so a state captured
    /// before an edit shortened the sequence still lands on a real frame.
    pub fn restore(
        &self,
        sequence: &Sequence,
        panel: &mut TimelinePanel,
        viewer: &mut ViewerState,
    ) {
        let rate = sequence.settings.frame_rate;
        let width_px = panel.view().width_px();
        let mut view = TimelineView::new(rate);
        view.set_width_px(width_px);
        view.set_zoom(self.zoom);
        view.set_scroll_px(self.scroll_px);
        *panel.view_mut() = view;
        panel.restore_lane_scroll(self.lane_scroll_px);
        panel.invalidate();

        viewer.set_rate(rate);
        viewer.set_duration(sequence_duration(sequence));
        viewer.seek_to(self.playhead);
    }
}

/// The "new sequence" dialog: a name and the settings the sequence is created
/// with.
///
/// The settings are validated by [`SequenceSettings::new`] before they can
/// leave the dialog, so a zero canvas dimension or a zero sample rate is
/// refused here rather than reaching a command.
#[derive(Debug, Clone)]
pub struct NewSequenceDialog {
    /// Whether the dialog is on screen.
    pub open: bool,
    /// The name the new sequence would get.
    pub name: String,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// Editing timebase.
    pub frame_rate: Rational,
    /// Audio sample rate in hertz.
    pub sample_rate: u32,
    /// Colour tags (decision-3: tags only, Rec.709 assumed).
    pub color: ColorTags,
}

impl Default for NewSequenceDialog {
    fn default() -> Self {
        let settings = SequenceSettings::default();
        Self {
            open: false,
            name: String::new(),
            width: settings.resolution.width(),
            height: settings.resolution.height(),
            frame_rate: settings.frame_rate,
            sample_rate: settings.sample_rate,
            color: settings.color,
        }
    }
}

impl NewSequenceDialog {
    /// A closed dialog holding the default sequence settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens the dialog, proposing `name`.
    pub fn open_with(&mut self, name: impl Into<String>) {
        *self = Self {
            open: true,
            name: name.into(),
            ..Self::default()
        };
    }

    /// The settings the dialog currently describes.
    ///
    /// # Errors
    ///
    /// `model.invalid_settings` when the canvas has a zero dimension or the
    /// sample rate is zero.
    pub fn settings(&self) -> SubResult<SequenceSettings> {
        let resolution = Resolution::new(self.width, self.height)?;
        SequenceSettings::new(resolution, self.frame_rate, self.sample_rate, self.color)
    }

    /// The name a created sequence would carry: what was typed, or
    /// `fallback` when nothing was.
    #[must_use]
    pub fn effective_name<'a>(&'a self, fallback: &'a str) -> &'a str {
        let trimmed = self.name.trim();
        if trimmed.is_empty() {
            fallback
        } else {
            trimmed
        }
    }

    /// Draws the dialog and reports a create request.
    ///
    /// Returns `None` while the dialog is closed, still being filled in, or
    /// cancelled.
    pub fn ui(&mut self, ctx: &egui::Context, fallback_name: &str) -> Option<SequenceTabAction> {
        if !self.open {
            return None;
        }
        let mut action = None;
        let mut open = true;
        egui::Window::new("New sequence")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                action = self.body_ui(ui, fallback_name);
            });
        if action.is_some() {
            open = false;
        }
        self.open = open;
        action
    }

    /// The dialog's contents, without the window frame, so the settings rows
    /// can be exercised headlessly.
    pub fn body_ui(&mut self, ui: &mut Ui, fallback_name: &str) -> Option<SequenceTabAction> {
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.name);
        });
        ui.horizontal(|ui| {
            ui.label("Canvas");
            for (label, resolution) in RESOLUTION_PRESETS {
                let chosen = self.width == resolution.width() && self.height == resolution.height();
                if ui.selectable_label(chosen, label).clicked() {
                    self.width = resolution.width();
                    self.height = resolution.height();
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Frame rate");
            for (label, rate) in FRAME_RATE_PRESETS {
                if ui
                    .selectable_label(self.frame_rate == rate, label)
                    .clicked()
                {
                    self.frame_rate = rate;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Sample rate");
            for rate in SAMPLE_RATE_PRESETS {
                let label = format!("{rate}");
                if ui
                    .selectable_label(self.sample_rate == rate, label)
                    .clicked()
                {
                    self.sample_rate = rate;
                }
            }
        });

        let settings = self.settings();
        if let Err(error) = &settings {
            ui.colored_label(egui::Color32::from_rgb(200, 64, 64), error.message.clone());
        }
        let mut action = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(settings.is_ok(), egui::Button::new("Create"))
                .clicked()
                && let Ok(settings) = settings
            {
                action = Some(SequenceTabAction::Create {
                    name: self.effective_name(fallback_name).to_owned(),
                    settings,
                });
            }
            if ui.button("Cancel").clicked() {
                self.open = false;
            }
        });
        action
    }
}

/// The sequence tab strip.
///
/// It knows which sequence is active and where each sequence was last being
/// looked at, and nothing else about the project.
#[derive(Debug, Clone, Default)]
pub struct SequenceTabs {
    /// The sequence on screen, once [`SequenceTabs::sync`] has seen one.
    active: Option<SequenceId>,
    /// Where each sequence was last looked at, including the active one as of
    /// the last switch.
    remembered: Vec<(SequenceId, SequenceViewState)>,
    /// The tab being renamed and the text typed so far.
    renaming: Option<(SequenceId, String)>,
    /// The new-sequence dialog.
    pub dialog: NewSequenceDialog,
}

impl SequenceTabs {
    /// An empty strip with no active sequence.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The sequence on screen.
    #[must_use]
    pub const fn active(&self) -> Option<SequenceId> {
        self.active
    }

    /// The sequence being renamed, if any.
    #[must_use]
    pub fn renaming(&self) -> Option<SequenceId> {
        self.renaming.as_ref().map(|(id, _)| *id)
    }

    /// Where `sequence` was last looked at, if it has been on screen.
    #[must_use]
    pub fn remembered(&self, sequence: SequenceId) -> Option<&SequenceViewState> {
        self.remembered
            .iter()
            .find(|(id, _)| *id == sequence)
            .map(|(_, state)| state)
    }

    /// Reconciles the strip with the project's sequences.
    ///
    /// Call this after every command: it activates the first sequence when
    /// none is active, moves off a sequence that has just been deleted, and
    /// forgets the view state of sequences that are gone. Returns the active
    /// sequence, which is `None` only for a project with no sequences.
    pub fn sync(&mut self, sequences: &[Sequence]) -> Option<SequenceId> {
        self.remembered
            .retain(|(id, _)| sequences.iter().any(|sequence| sequence.id == *id));
        if self
            .renaming
            .as_ref()
            .is_some_and(|(id, _)| !sequences.iter().any(|sequence| sequence.id == *id))
        {
            self.renaming = None;
        }
        let still_there = self
            .active
            .is_some_and(|active| sequences.iter().any(|sequence| sequence.id == active));
        if !still_there {
            self.active = sequences.first().map(|sequence| sequence.id);
        }
        self.active
    }

    /// Switches to `sequence`, saving where the outgoing tab was being looked
    /// at and restoring where the incoming one was.
    ///
    /// A sequence that has never been on screen starts at
    /// [`SequenceViewState::initial`].
    ///
    /// # Errors
    ///
    /// `ui.unknown_sequence` when `sequence` is not one of `sequences`.
    pub fn switch_to(
        &mut self,
        sequences: &[Sequence],
        sequence: SequenceId,
        panel: &mut TimelinePanel,
        viewer: &mut ViewerState,
    ) -> SubResult<()> {
        let target = Self::find(sequences, sequence)?;
        if let Some(active) = self.active {
            self.remember(active, SequenceViewState::capture(panel, viewer));
        }
        let state = self
            .remembered(sequence)
            .copied()
            .unwrap_or_else(|| SequenceViewState::initial(target));
        state.restore(target, panel, viewer);
        self.remember(sequence, state);
        self.active = Some(sequence);
        Ok(())
    }

    /// Whether deleting `sequence` is allowed.
    ///
    /// # Errors
    ///
    /// `ui.unknown_sequence` when `sequence` is not one of `sequences`, and
    /// `ui.last_sequence` when it is the only one left: a project always has
    /// a sequence to edit, so the last tab cannot be closed. The
    /// `DeleteSequence` command itself stays permissive, because it is also
    /// the inverse of creating the first sequence.
    pub fn check_delete(sequences: &[Sequence], sequence: SequenceId) -> SubResult<()> {
        Self::find(sequences, sequence)?;
        if sequences.len() <= 1 {
            return Err(SubError::new(
                codes::LAST_SEQUENCE,
                "a project must keep at least one sequence",
            )
            .with_detail("sequence_id", sequence));
        }
        Ok(())
    }

    /// Starts renaming `sequence`, seeding the editor with its current name.
    ///
    /// # Errors
    ///
    /// `ui.unknown_sequence` when `sequence` is not one of `sequences`.
    pub fn begin_rename(&mut self, sequences: &[Sequence], sequence: SequenceId) -> SubResult<()> {
        let found = Self::find(sequences, sequence)?;
        self.renaming = Some((sequence, found.name.clone()));
        Ok(())
    }

    /// Abandons a rename in progress, leaving the name as it was.
    pub fn cancel_rename(&mut self) {
        self.renaming = None;
    }

    /// Ends the rename in progress and reports the command to run.
    ///
    /// An empty or unchanged name is no edit at all, so it yields `None`
    /// rather than an action that would push a no-op onto the undo stack.
    pub fn commit_rename(&mut self, sequences: &[Sequence]) -> Option<SequenceTabAction> {
        let (sequence, typed) = self.renaming.take()?;
        let name = typed.trim();
        let found = Self::find(sequences, sequence).ok()?;
        if name.is_empty() || name == found.name {
            return None;
        }
        Some(SequenceTabAction::Rename {
            sequence,
            name: name.to_owned(),
        })
    }

    /// Draws the tab strip and the new-sequence dialog, and reports what the
    /// user asked for.
    ///
    /// Draw it above the timeline panel: the strip lays itself out in one
    /// horizontal row and leaves the rest of the `Ui` to the panel. At most
    /// one action comes out of a frame.
    pub fn ui(&mut self, ui: &mut Ui, sequences: &[Sequence]) -> Option<SequenceTabAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            for sequence in sequences {
                if let Some(next) = self.tab_ui(ui, sequences, sequence) {
                    action = Some(next);
                }
            }
            if ui.button("+").on_hover_text("New sequence").clicked() {
                self.dialog.open_with(default_sequence_name(sequences));
            }
        });
        let created = self.dialog.ui(ui.ctx(), &default_sequence_name(sequences));
        action.or(created)
    }

    /// One tab: a label that selects, a rename editor while renaming, and a
    /// context menu carrying rename and delete.
    fn tab_ui(
        &mut self,
        ui: &mut Ui,
        sequences: &[Sequence],
        sequence: &Sequence,
    ) -> Option<SequenceTabAction> {
        if let Some((renaming, text)) = self.renaming.as_mut()
            && *renaming == sequence.id
        {
            let response = ui.add(
                egui::TextEdit::singleline(text)
                    .id(egui::Id::new(("sequence-tab-rename", sequence.id)))
                    .desired_width(96.0),
            );
            response.request_focus();
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                self.cancel_rename();
                return None;
            }
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                return self.commit_rename(sequences);
            }
            return None;
        }

        let active = self.active == Some(sequence.id);
        let response = ui.selectable_label(active, &sequence.name);
        let mut action = None;
        if response.double_clicked() {
            let _ = self.begin_rename(sequences, sequence.id);
        } else if response.clicked() && !active {
            action = Some(SequenceTabAction::Switch(sequence.id));
        }
        response.context_menu(|ui| {
            if ui.button("Rename").clicked() {
                let _ = self.begin_rename(sequences, sequence.id);
                ui.close();
            }
            let deletable = Self::check_delete(sequences, sequence.id);
            let delete = ui.add_enabled(deletable.is_ok(), egui::Button::new("Delete"));
            let delete = match &deletable {
                Ok(()) => delete,
                Err(error) => delete.on_disabled_hover_text(error.message.clone()),
            };
            if delete.clicked() {
                action = Some(SequenceTabAction::Delete(sequence.id));
                ui.close();
            }
        });
        action
    }

    /// Records `state` as where `sequence` is being looked at.
    fn remember(&mut self, sequence: SequenceId, state: SequenceViewState) {
        if let Some(slot) = self
            .remembered
            .iter_mut()
            .find(|(id, _)| *id == sequence)
            .map(|(_, state)| state)
        {
            *slot = state;
        } else {
            self.remembered.push((sequence, state));
        }
    }

    /// The sequence with `id`, or `ui.unknown_sequence`.
    fn find(sequences: &[Sequence], id: SequenceId) -> SubResult<&Sequence> {
        sequences
            .iter()
            .find(|sequence| sequence.id == id)
            .ok_or_else(|| {
                SubError::new(codes::UNKNOWN_SEQUENCE, "no such sequence in this project")
                    .with_detail("sequence_id", id)
            })
    }
}

/// A name for the next sequence: "Sequence 1", "Sequence 2" and so on, past
/// whatever is already taken.
#[must_use]
pub fn default_sequence_name(sequences: &[Sequence]) -> String {
    for index in 1..=sequences.len().saturating_add(1) {
        let candidate = format!("Sequence {index}");
        if !sequences.iter().any(|sequence| sequence.name == candidate) {
            return candidate;
        }
    }
    "Sequence".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_model::{Track, TrackKind};

    fn sequence(name: &str, rate: Rational) -> Sequence {
        let settings = SequenceSettings::new(Resolution::HD_1080, rate, 48_000, ColorTags::REC709)
            .expect("valid settings");
        let mut sequence = Sequence::new(name, settings);
        sequence.tracks.push(Track::new("V1", TrackKind::Video));
        sequence
    }

    #[test]
    fn sync_activates_the_first_sequence_and_drops_deleted_ones() {
        let sequences = vec![
            sequence("A", Rational::FPS_24),
            sequence("B", Rational::FPS_25),
        ];
        let mut tabs = SequenceTabs::new();
        assert_eq!(tabs.sync(&sequences), Some(sequences[0].id));

        let mut panel = TimelinePanel::new(Rational::FPS_24);
        let mut viewer = ViewerState::new(Rational::FPS_24);
        tabs.switch_to(&sequences, sequences[1].id, &mut panel, &mut viewer)
            .expect("sequence exists");
        assert_eq!(tabs.active(), Some(sequences[1].id));

        let remaining = vec![sequences[0].clone()];
        assert_eq!(tabs.sync(&remaining), Some(sequences[0].id));
        assert!(tabs.remembered(sequences[1].id).is_none());
    }

    #[test]
    fn switching_to_an_unknown_sequence_is_refused() {
        let sequences = vec![sequence("A", Rational::FPS_24)];
        let mut tabs = SequenceTabs::new();
        tabs.sync(&sequences);
        let mut panel = TimelinePanel::new(Rational::FPS_24);
        let mut viewer = ViewerState::new(Rational::FPS_24);
        let err = tabs
            .switch_to(&sequences, SequenceId::new(), &mut panel, &mut viewer)
            .unwrap_err();
        assert_eq!(err.code, codes::UNKNOWN_SEQUENCE);
    }

    #[test]
    fn deleting_the_last_sequence_is_refused() {
        let sequences = vec![sequence("A", Rational::FPS_24)];
        let err = SequenceTabs::check_delete(&sequences, sequences[0].id).unwrap_err();
        assert_eq!(err.code, codes::LAST_SEQUENCE);

        let two = vec![sequences[0].clone(), sequence("B", Rational::FPS_24)];
        SequenceTabs::check_delete(&two, two[0].id).expect("two sequences, one can go");
        let err = SequenceTabs::check_delete(&two, SequenceId::new()).unwrap_err();
        assert_eq!(err.code, codes::UNKNOWN_SEQUENCE);
    }

    #[test]
    fn a_rename_only_yields_a_command_when_the_name_actually_changed() {
        let sequences = vec![sequence("A", Rational::FPS_24)];
        let mut tabs = SequenceTabs::new();
        tabs.sync(&sequences);
        tabs.begin_rename(&sequences, sequences[0].id)
            .expect("known");
        assert_eq!(tabs.renaming(), Some(sequences[0].id));
        assert_eq!(tabs.commit_rename(&sequences), None, "unchanged name");

        tabs.begin_rename(&sequences, sequences[0].id)
            .expect("known");
        tabs.renaming.as_mut().expect("renaming").1 = "  ".to_owned();
        assert_eq!(tabs.commit_rename(&sequences), None, "blank name");

        tabs.begin_rename(&sequences, sequences[0].id)
            .expect("known");
        tabs.renaming.as_mut().expect("renaming").1 = " Programme ".to_owned();
        assert_eq!(
            tabs.commit_rename(&sequences),
            Some(SequenceTabAction::Rename {
                sequence: sequences[0].id,
                name: "Programme".to_owned(),
            })
        );
        assert_eq!(tabs.renaming(), None);
    }

    #[test]
    fn the_dialog_validates_its_settings_before_they_leave() {
        let mut dialog = NewSequenceDialog::new();
        dialog.open_with("Sequence 2");
        assert!(dialog.open);
        assert_eq!(dialog.effective_name("fallback"), "Sequence 2");
        let settings = dialog.settings().expect("defaults are valid");
        assert_eq!(settings, SequenceSettings::default());

        dialog.height = 0;
        assert!(dialog.settings().is_err(), "a zero canvas is refused");
        dialog.height = 2160;
        dialog.sample_rate = 0;
        assert!(dialog.settings().is_err(), "a zero sample rate is refused");

        dialog.sample_rate = 44_100;
        dialog.frame_rate = Rational::FPS_29_97;
        let settings = dialog.settings().expect("valid");
        assert_eq!(settings.frame_rate.numerator(), 30_000);
        assert_eq!(settings.frame_rate.denominator(), 1001);

        dialog.name = "   ".to_owned();
        assert_eq!(dialog.effective_name("Sequence 7"), "Sequence 7");
    }

    #[test]
    fn the_default_name_skips_the_ones_already_taken() {
        let sequences = vec![sequence("Sequence 1", Rational::FPS_24)];
        assert_eq!(default_sequence_name(&sequences), "Sequence 2");
        assert_eq!(default_sequence_name(&[]), "Sequence 1");
    }
}
