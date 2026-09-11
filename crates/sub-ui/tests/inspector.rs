//! Driving the inspector through the shared `egui_kittest` harness.
//!
//! The panel raises commands rather than mutating anything, so the tests here
//! hold a project and a [`History`] beside it and apply what the panel raised
//! through [`sub_ui::inspector::apply_edit`] — which is what the app does.
//! That makes the assertions the real contract: the project changes while the
//! pointer is still down (live), and the whole gesture is one entry in the
//! undo stack (committed on release).
//!
//! The snapshot paints the panel over the committed sample project, so a
//! change to the fixture or to the field list shows up as a visible diff.

mod support;

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use sub_edit::History;
use sub_edit::commands::{AddClipEffect, MoveClipEffect, RemoveClipEffect, SetClipEffectParam};
use sub_model::effect::{ClipEffect, EffectValue};
use sub_model::params::{Fixed6, Opacity};
use sub_model::sequence::SequenceSettings;
use sub_model::{
    Clip, ClipId, MediaItem, MediaPath, Project, Sequence, Track, TrackItem, TrackKind,
};
use sub_render::{EffectParam, ParamKind};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::effects::{EffectCatalog, EffectListing};
use sub_ui::inspector::{EffectEdit, InspectorField, InspectorPanel, apply_edit};
use sub_ui::selection::{ClipRef, Selection};

/// The timebase every scene here uses.
const RATE: Rational = Rational::FPS_24;

/// Everything a painted frame needs, and everything it is asserted on.
struct Scene {
    panel: InspectorPanel,
    project: Project,
    history: History,
    selection: Selection,
    catalog: EffectCatalog,
}

impl Scene {
    /// The clips of the first track, in order.
    fn clips(&self) -> Vec<&Clip> {
        self.project.sequences[0].tracks[0].clips().collect()
    }

    /// The opacity of the clip at `index`, in micro-units.
    fn opacity_micros(&self, index: usize) -> i64 {
        self.clips()[index].opacity.factor().micros()
    }
}

/// A project with one video track holding `count` 48-frame clips, with the
/// first `selected` of them selected.
fn scene(count: usize, selected: usize) -> Scene {
    let mut project = Project::new("inspector");
    let item = MediaItem::new(MediaPath::new("media/take.mp4").expect("valid path"));
    let media = item.id;
    project.media.push(item);

    let mut track = Track::new("V1", TrackKind::Video);
    for index in 0..count {
        let source = TimeRange::new(RationalTime::new(0, RATE), RationalTime::new(48, RATE))
            .expect("valid source range");
        track.items.push(TrackItem::Clip(Clip::new(
            format!("take {index}"),
            media,
            source,
        )));
    }
    let track_id = track.id;
    let chosen: Vec<ClipId> = track.clips().take(selected).map(|clip| clip.id).collect();
    let mut sequence = Sequence::new("edit", SequenceSettings::default());
    sequence.tracks.push(track);
    project.sequences.push(sequence);

    let mut selection = Selection::new();
    for clip in chosen {
        selection.toggle(ClipRef::new(track_id, clip));
    }
    Scene {
        panel: InspectorPanel::new(),
        project,
        history: History::new(),
        selection,
        catalog: catalog(),
    }
}

/// A harness painting the inspector over `scene`, applying what it raises.
fn harness(scene: Scene) -> Harness<'static, Scene> {
    support::panel_harness_state(scene, |ui, scene| {
        // The panel reads the sequence and the commands write to the project,
        // so the sequence is taken first and the edit applied afterwards.
        let sequence = scene.project.sequences[0].clone();
        let response = scene
            .panel
            .ui(ui, &sequence, &scene.selection, &scene.catalog);
        apply_edit(&mut scene.history, &mut scene.project, &response)
            .expect("the inspector's edit applies");
    })
}

/// The rectangle of the slider track labelled `label`.
///
/// A slider publishes two accessible nodes carrying the same label: the track
/// itself and the number beside it. The track is the first of them, and it is
/// what a drag aims at.
fn slider_rect<State>(harness: &Harness<'_, State>, label: &str) -> egui::Rect {
    harness
        .get_all_by_label(label)
        .next()
        .expect("the inspector paints this field")
        .rect()
}

/// Whether anything in the panel carries `label`.
fn shows<State>(harness: &Harness<'_, State>, label: &str) -> bool {
    harness.query_all_by_label(label).next().is_some()
}

#[test]
fn every_parameter_field_is_shown_for_a_selected_clip() {
    let mut harness = harness(scene(1, 1));
    harness.run();
    for field in InspectorField::ALL {
        assert!(
            shows(&harness, field.label()),
            "the inspector shows {:?}",
            field.label()
        );
    }
}

#[test]
fn an_empty_selection_shows_a_prompt_and_no_fields() {
    let mut harness = harness(scene(1, 0));
    harness.run();
    assert!(
        shows(&harness, "Select a clip to edit its parameters."),
        "the inspector says what to do with nothing selected"
    );
    assert!(
        !shows(&harness, InspectorField::Opacity.label()),
        "and offers no fields to edit"
    );
}

#[test]
fn dragging_the_opacity_slider_edits_live_and_commits_one_undo_step() {
    let mut harness = harness(scene(1, 1));
    harness.run();
    assert_eq!(
        harness.state().opacity_micros(0),
        1_000_000,
        "starts opaque"
    );

    let rect = slider_rect(&harness, InspectorField::Opacity.label());
    // The handle sits at the right-hand end, because the clip is opaque.
    let handle = egui::pos2(rect.right() - 4.0, rect.center().y);
    harness.hover_at(handle);
    harness.run();
    harness.drag_at(handle);
    harness.run();

    // Half way along the track, still holding the button down.
    let middle = rect.center();
    harness.hover_at(middle);
    harness.run();
    let live = harness.state().opacity_micros(0);
    assert!(
        live < 1_000_000,
        "the drag changed the clip while the pointer is still down: {live}"
    );
    assert_eq!(
        harness.state().history.undo_entries().count(),
        0,
        "and has not pushed an undo entry yet"
    );

    harness.drop_at(middle);
    harness.run();
    let committed = harness.state().opacity_micros(0);
    assert_eq!(
        committed, live,
        "releasing keeps the value the drag reached"
    );
    assert_eq!(
        harness.state().history.undo_entries().count(),
        1,
        "the whole drag is one entry in the undo stack"
    );

    let Scene {
        project, history, ..
    } = harness.state_mut();
    history.undo(project).expect("the drag undoes");
    assert_eq!(
        project.sequences[0].tracks[0]
            .clips()
            .next()
            .expect("a clip")
            .opacity,
        Opacity::OPAQUE,
        "one undo puts the whole drag back"
    );
}

#[test]
fn the_gain_field_shows_decibels_and_edits_the_clip_in_them() {
    let mut harness = harness(scene(1, 1));
    harness.run();
    assert_eq!(
        harness.state().clips()[0].gain,
        sub_model::GainDb::UNITY,
        "a fresh clip plays at unity"
    );
    assert!(
        harness
            .query_all_by_label(InspectorField::Gain.label())
            .any(|node| node.value().is_some_and(|value| value.ends_with(" dB"))),
        "the gain field reads out in decibels"
    );

    // The fader runs from -144 dB to +24 dB, so a quarter of the way along is
    // well below unity.
    let rect = slider_rect(&harness, InspectorField::Gain.label());
    let target = egui::pos2(rect.left() + rect.width() * 0.25, rect.center().y);
    harness.hover_at(target);
    harness.run();
    harness.drag_at(target);
    harness.run();
    harness.drop_at(target);
    harness.run();

    let quieter = harness.state().clips()[0].gain;
    assert!(
        quieter.decibels().as_f64() < 0.0,
        "dragging down the fader cut the clip's level: {quieter:?}"
    );
    assert_eq!(
        harness.state().history.undo_entries().count(),
        1,
        "the whole gesture is one entry in the undo stack"
    );

    let Scene {
        project, history, ..
    } = harness.state_mut();
    history.undo(project).expect("the gain edit undoes");
    assert_eq!(
        project.sequences[0].tracks[0]
            .clips()
            .next()
            .expect("a clip")
            .gain,
        sub_model::GainDb::UNITY,
        "one undo puts the level back"
    );
}

#[test]
fn an_edit_applies_to_every_selected_clip() {
    let mut harness = harness(scene(3, 3));
    harness.run();

    let rect = slider_rect(&harness, InspectorField::Opacity.label());
    let target = egui::pos2(rect.left() + rect.width() * 0.25, rect.center().y);
    harness.hover_at(target);
    harness.run();
    harness.drag_at(target);
    harness.run();
    harness.drop_at(target);
    harness.run();

    let edited: Vec<i64> = (0..3)
        .map(|index| harness.state().opacity_micros(index))
        .collect();
    assert!(
        edited.iter().all(|micros| *micros < 1_000_000),
        "every selected clip was edited: {edited:?}"
    );
    assert!(
        edited.windows(2).all(|pair| pair[0] == pair[1]),
        "and all to the same value: {edited:?}"
    );
    assert_eq!(
        harness.state().history.undo_entries().count(),
        1,
        "the multi-clip edit is still one undo step"
    );

    let Scene {
        project, history, ..
    } = harness.state_mut();
    history.undo(project).expect("the edit undoes");
    for clip in project.sequences[0].tracks[0].clips() {
        assert_eq!(clip.opacity, Opacity::OPAQUE, "every clip went back");
    }
}

#[test]
fn the_command_the_panel_issues_names_the_selected_clip_and_only_the_edited_field() {
    let mut scene = scene(1, 1);
    let sequence = scene.project.sequences[0].clone();
    let clip = sequence.tracks[0].clips().next().expect("a clip").id;
    let track = sequence.tracks[0].id;

    let mut raised = Vec::new();
    let mut begins = Vec::new();
    let mut commits = 0_usize;
    let mut harness = support::panel_harness_state(
        (&mut scene, &mut raised, &mut begins, &mut commits),
        |ui, (scene, raised, begins, commits)| {
            let sequence = scene.project.sequences[0].clone();
            let response = scene
                .panel
                .ui(ui, &sequence, &scene.selection, &scene.catalog);
            raised.extend(response.commands.iter().copied());
            if let Some(label) = response.begin.clone() {
                begins.push(label);
            }
            if response.commit {
                **commits += 1;
            }
            apply_edit(&mut scene.history, &mut scene.project, &response)
                .expect("the inspector's edit applies");
        },
    );
    harness.run();

    let rect = slider_rect(&harness, InspectorField::Opacity.label());
    let target = egui::pos2(rect.left() + 2.0, rect.center().y);
    harness.hover_at(target);
    harness.run();
    harness.drag_at(target);
    harness.run();
    harness.drop_at(target);
    harness.run();
    drop(harness);

    assert!(!raised.is_empty(), "the drag issued commands");
    let first = raised[0];
    assert_eq!(first.sequence, sequence.id);
    assert_eq!(first.track, track);
    assert_eq!(first.clip, clip);
    assert_eq!(
        first
            .opacity
            .expect("the command names the opacity")
            .factor(),
        Fixed6::ZERO,
        "dragging to the left-hand end sets the opacity to nothing"
    );
    assert!(
        first.transform.is_none() && first.gain.is_none(),
        "and names no other parameter"
    );
    assert_eq!(
        begins,
        vec![InspectorField::Opacity.undo_label().to_owned()],
        "one gesture opened one history group"
    );
    assert_eq!(commits, 1, "and it was committed once");
}

#[test]
fn a_locked_track_offers_nothing_to_edit() {
    let mut scene = scene(1, 1);
    scene.project.sequences[0].tracks[0].locked = true;
    let mut harness = harness(scene);
    harness.run();
    assert!(
        shows(&harness, "Every selected clip is on a locked track."),
        "the inspector says why the fields are dead"
    );
    assert_eq!(
        harness.state().opacity_micros(0),
        1_000_000,
        "and nothing was edited"
    );
}

#[test]
fn the_inspector_matches_its_snapshot_over_the_sample_project() {
    if !support::can_render() {
        return;
    }
    let project = support::fixture_project();
    let sequence = support::fixture_sequence(&project).clone();
    let track = sequence
        .tracks
        .iter()
        .find(|track| track.clips().next().is_some())
        .expect("the sample project has a clip");
    let clip = track.clips().next().expect("a clip").id;
    let mut selection = Selection::new();
    selection.toggle(ClipRef::new(track.id, clip));

    let catalog = catalog();
    let mut panel = InspectorPanel::new();
    let mut harness = support::panel_harness(|ui| {
        panel.ui(ui, &sequence, &selection, &catalog);
    });
    harness.run();
    support::snapshot(&mut harness, "inspector_clip_parameters");
}

/// The plugin ids the catalogue offers.
const GRADE: &str = "com.example.grade";
const BLUR: &str = "com.example.blur";

/// A catalogue of two installed effect plugins.
///
/// This is what the plugin host hands the inspector once it has loaded the
/// installed `effect`-world plugins: an id, a name and the parameters the
/// plugin declared, which is all the panel needs to build controls.
fn catalog() -> EffectCatalog {
    let mut catalog = EffectCatalog::new();
    catalog.push(EffectListing::new(GRADE, "Grade").with_params(vec![
        EffectParam::new(
            "exposure",
            "Exposure",
            ParamKind::Float {
                min: -4.0,
                max: 4.0,
                default: 0.0,
                step: None,
            },
        ),
        EffectParam::new("invert", "Invert", ParamKind::Bool { default: false }),
    ]));
    catalog.push(
        EffectListing::new(BLUR, "Blur").with_params(vec![EffectParam::new(
            "radius",
            "Radius",
            ParamKind::Int {
                min: 0,
                max: 32,
                default: 4,
            },
        )]),
    );
    catalog
}

/// One clip, selected, with `plugins` applied to it and an empty undo stack.
fn scene_with_effects(plugins: &[&str]) -> Scene {
    let mut scene = scene(1, 1);
    let sequence = scene.project.sequences[0].id;
    let track = scene.project.sequences[0].tracks[0].id;
    let clip = scene.clips()[0].id;
    for plugin in plugins {
        scene
            .history
            .apply(
                &mut scene.project,
                AddClipEffect::new(sequence, track, clip, *plugin),
            )
            .expect("the effect applies");
    }
    scene.history.clear();
    scene
}

/// The plugin ids on the clip's effect stack, in order.
fn stack(scene: &Scene) -> Vec<String> {
    scene.clips()[0]
        .effects
        .iter()
        .map(|effect| effect.plugin.clone())
        .collect()
}

/// The clip's effect stack after the harness has finished with it.
fn effects_of(project: &Project) -> Vec<ClipEffect> {
    project.sequences[0].tracks[0]
        .clips()
        .next()
        .expect("a clip")
        .effects
        .clone()
}

#[test]
fn the_effect_list_shows_every_applied_effect_with_its_declared_controls() {
    let mut harness = harness(scene_with_effects(&[GRADE, BLUR]));
    harness.run();

    assert!(shows(&harness, "Effects"), "the stack has a heading");
    assert!(shows(&harness, "1. Grade"), "in the order it runs in");
    assert!(shows(&harness, "2. Blur"));
    assert!(
        shows(&harness, "Exposure") && shows(&harness, "Invert"),
        "with a control per parameter the plugin declared"
    );
    assert!(shows(&harness, "Radius"));
    assert!(
        shows(&harness, "Grade") && shows(&harness, "Blur"),
        "and the picker offers every installed effect plugin"
    );
}

#[test]
fn a_clip_with_no_effects_says_so_and_still_offers_the_picker() {
    let mut harness = harness(scene_with_effects(&[]));
    harness.run();
    assert!(shows(&harness, "No effects on this clip."));
    assert!(shows(&harness, "Add effect"));
    assert!(shows(&harness, "Grade"));
}

#[test]
fn adding_an_effect_from_the_picker_is_one_undoable_command() {
    let mut harness = harness(scene_with_effects(&[]));
    harness.run();
    harness.get_by_label("Blur").click();
    harness.run();

    assert_eq!(stack(harness.state()), vec![BLUR.to_owned()]);
    assert_eq!(
        harness.state().history.undo_entries().count(),
        1,
        "the click is one entry in the undo stack"
    );

    let Scene {
        project, history, ..
    } = harness.state_mut();
    history.undo(project).expect("the add undoes");
    assert!(
        effects_of(project).is_empty(),
        "and undo takes the effect off again"
    );
}

#[test]
fn moving_an_effect_down_reorders_the_stack_and_undoes() {
    let mut harness = harness(scene_with_effects(&[GRADE, BLUR]));
    harness.run();
    // Both rows paint the button; the last row's is disabled, so the first
    // one is the one that moves Grade under Blur.
    harness
        .get_all_by_label("Move down")
        .next()
        .expect("the first effect can move down")
        .click();
    harness.run();

    assert_eq!(
        stack(harness.state()),
        vec![BLUR.to_owned(), GRADE.to_owned()]
    );
    assert_eq!(harness.state().history.undo_entries().count(), 1);

    let Scene {
        project, history, ..
    } = harness.state_mut();
    history.undo(project).expect("the move undoes");
    assert_eq!(
        effects_of(project)
            .into_iter()
            .map(|effect| effect.plugin)
            .collect::<Vec<_>>(),
        vec![GRADE.to_owned(), BLUR.to_owned()]
    );
}

#[test]
fn removing_an_effect_takes_it_off_and_undoes_with_its_values() {
    let mut scene = scene_with_effects(&[GRADE, BLUR]);
    let sequence = scene.project.sequences[0].id;
    let track = scene.project.sequences[0].tracks[0].id;
    let clip = scene.clips()[0].id;
    let grade = scene.clips()[0].effects[0].id;
    scene
        .history
        .apply(
            &mut scene.project,
            SetClipEffectParam::new(
                sequence,
                track,
                clip,
                grade,
                "exposure",
                EffectValue::Float(Fixed6::ONE),
            ),
        )
        .expect("the parameter binds");
    scene.history.clear();

    let mut harness = harness(scene);
    harness.run();
    harness
        .get_all_by_label("Remove")
        .next()
        .expect("the first effect has a remove button")
        .click();
    harness.run();

    assert_eq!(stack(harness.state()), vec![BLUR.to_owned()]);
    assert_eq!(harness.state().history.undo_entries().count(), 1);

    let Scene {
        project, history, ..
    } = harness.state_mut();
    history.undo(project).expect("the removal undoes");
    let effects = effects_of(project);
    assert_eq!(
        effects[0].id, grade,
        "the effect goes back with its identity"
    );
    assert_eq!(
        effects[0].param("exposure"),
        Some(EffectValue::Float(Fixed6::ONE)),
        "and everything bound on it"
    );
}

#[test]
fn the_effect_commands_the_panel_issues_name_the_clip_and_the_effect() {
    let mut scene = scene_with_effects(&[GRADE, BLUR]);
    let sequence = scene.project.sequences[0].id;
    let track = scene.project.sequences[0].tracks[0].id;
    let clip = scene.clips()[0].id;
    let grade = scene.clips()[0].effects[0].id;
    let blur = scene.clips()[0].effects[1].id;

    let mut raised: Vec<EffectEdit> = Vec::new();
    let catalog = catalog();
    let mut harness =
        support::panel_harness_state((&mut scene, &mut raised), |ui, (scene, raised)| {
            let sequence = scene.project.sequences[0].clone();
            let response = scene.panel.ui(ui, &sequence, &scene.selection, &catalog);
            raised.extend(response.effects.iter().cloned());
            apply_edit(&mut scene.history, &mut scene.project, &response)
                .expect("the inspector's edit applies");
        });
    harness.run();
    // Grade moves under Blur, Blur (now the first row) comes off, and Grade is
    // applied a second time from the picker.
    harness
        .get_all_by_label("Move down")
        .next()
        .expect("the first effect can move down")
        .click();
    harness.run();
    harness
        .get_all_by_label("Remove")
        .next()
        .expect("a remove button")
        .click();
    harness.run();
    harness.get_by_label("Grade").click();
    harness.run();
    drop(harness);

    assert_eq!(
        raised,
        vec![
            EffectEdit::Move(MoveClipEffect::new(sequence, track, clip, grade, 1)),
            EffectEdit::Remove(RemoveClipEffect::new(sequence, track, clip, blur)),
            EffectEdit::Add(AddClipEffect::new(sequence, track, clip, GRADE)),
        ],
        "each click raised exactly the command it names"
    );
    assert_eq!(
        scene.history.undo_entries().count(),
        3,
        "and each is one entry in the undo stack"
    );
}

#[test]
fn dragging_an_effect_parameter_edits_live_and_commits_one_undo_step() {
    let mut harness = harness(scene_with_effects(&[GRADE]));
    harness.run();

    let rect = slider_rect(&harness, "Exposure");
    let target = egui::pos2(rect.left() + rect.width() * 0.25, rect.center().y);
    harness.hover_at(target);
    harness.run();
    harness.drag_at(target);
    harness.run();

    let live = harness.state().clips()[0].effects[0].param("exposure");
    assert!(
        live.is_some(),
        "the drag bound the parameter while the pointer is still down"
    );
    assert_eq!(
        harness.state().history.undo_entries().count(),
        0,
        "and has not pushed an undo entry yet"
    );

    harness.drop_at(target);
    harness.run();
    assert_eq!(
        harness.state().history.undo_entries().count(),
        1,
        "the whole drag is one entry in the undo stack"
    );

    let Scene {
        project, history, ..
    } = harness.state_mut();
    history.undo(project).expect("the drag undoes");
    assert_eq!(
        effects_of(project)[0].param("exposure"),
        None,
        "one undo puts the parameter back to the plugin's declared default"
    );
}

#[test]
fn the_effect_list_matches_its_snapshot_over_the_sample_project() {
    if !support::can_render() {
        return;
    }
    let mut project = support::fixture_project();
    let sequence_id = support::fixture_sequence(&project).id;
    let track = support::fixture_sequence(&project)
        .tracks
        .iter()
        .find(|track| track.clips().next().is_some())
        .expect("the sample project has a clip");
    let track_id = track.id;
    let clip = track.clips().next().expect("a clip").id;

    let mut history = History::new();
    for plugin in [GRADE, BLUR] {
        history
            .apply(
                &mut project,
                AddClipEffect::new(sequence_id, track_id, clip, plugin),
            )
            .expect("the effect applies");
    }

    let sequence = support::fixture_sequence(&project).clone();
    let mut selection = Selection::new();
    selection.toggle(ClipRef::new(track_id, clip));
    let catalog = catalog();
    let mut panel = InspectorPanel::new();
    let mut harness = support::panel_harness(|ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            panel.ui(ui, &sequence, &selection, &catalog);
        });
    });
    harness.run();
    support::snapshot(&mut harness, "inspector_clip_effects");
}
