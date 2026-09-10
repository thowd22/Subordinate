//! The generated host bindings are implementable and linkable.
//!
//! TASK-84 owns the real wasmtime host; what this proves now is that the WIT in
//! `wit/subordinate-plugin.wit` produces a `Host` trait a plain in-memory
//! project can satisfy, that every accessor is answerable from `sub-model`
//! through [`sub_plugin`]'s conversions, and that the world links into a
//! [`wasmtime::component::Linker`] with no imports left unsatisfied.

use sub_core::{ErrorCode, SubError};
use sub_model::{
    Clip, ColorTags, Marker, MediaItem, MediaPath, Project, ProjectId, Resolution, Sequence,
    SequenceSettings, Track, TrackItem, TrackKind,
};
use sub_plugin::command_api::{
    ClipMetadata, Host, LogLevel, MarkerMetadata, ProjectMetadata, SequenceMetadata, TrackMetadata,
};
use sub_plugin::{
    Command, Commands, WitError, WitProjectId, WitSequenceId, WitTrackId, marker_metadata,
    project_metadata, sequence_metadata, track_clip_metadata, track_metadata,
};
use sub_time::{Rational, RationalTime, TimeRange};

/// The rate the fixture sequence runs at: not an integer, on purpose.
fn fps() -> Rational {
    Rational::FPS_23_976
}

/// A host that serves the interface out of one open project.
struct TestHost {
    project: Project,
    revision: u64,
    playhead: RationalTime,
    log: Vec<(LogLevel, String)>,
}

impl TestHost {
    fn new() -> Self {
        let settings =
            SequenceSettings::new(Resolution::HD_1080, fps(), 48_000, ColorTags::default())
                .unwrap();
        let mut sequence = Sequence::new("Seq 1", settings);
        let media = MediaItem::new(MediaPath::new("media/clip.mov").unwrap());
        let mut track = Track::new("V1", TrackKind::Video);
        let source = TimeRange::new(
            RationalTime::zero(fps()),
            RationalTime::from_frames(24, fps()),
        )
        .unwrap();
        track
            .items
            .push(TrackItem::Clip(Clip::new("A", media.id, source)));
        sequence.tracks.push(track);
        sequence.markers.push(Marker::new(
            "sync",
            TimeRange::empty_at(RationalTime::from_frames(12, fps())),
        ));

        let mut project = Project::new("Fixture");
        project.media.push(media);
        project.sequences.push(sequence);

        Self {
            project,
            revision: 3,
            playhead: RationalTime::from_frames(12, fps()),
            log: Vec::new(),
        }
    }

    /// Rejects a call naming a project this host does not have open.
    fn check(&self, project: &WitProjectId) -> Result<(), WitError> {
        let id = ProjectId::try_from(project).map_err(WitError::from)?;
        if id == self.project.id {
            return Ok(());
        }
        Err(SubError::new(
            ErrorCode::from_static("plugin.no_such_project"),
            "no such project",
        )
        .into())
    }

    /// The one sequence `sequence` names, or `plugin.no_such_sequence`.
    fn sequence(&self, sequence: &WitSequenceId) -> Result<&Sequence, WitError> {
        let id = sub_model::SequenceId::try_from(sequence).map_err(WitError::from)?;
        self.project.sequence(id).ok_or_else(|| {
            SubError::new(
                ErrorCode::from_static("plugin.no_such_sequence"),
                "no such sequence",
            )
            .into()
        })
    }
}

impl sub_plugin::types::Host for TestHost {}

/// The registration records carry no host functions of their own; a host still
/// declares that it serves the interface they live in.
impl sub_plugin::command_menu::Host for TestHost {}

impl Host for TestHost {
    fn run_command(
        &mut self,
        project: WitProjectId,
        method: String,
        _params: String,
    ) -> Result<String, WitError> {
        self.check(&project)?;
        // A real host dispatches into sub-command here; the point under test is
        // the shape of the boundary, not the dispatcher.
        Ok(format!(r#"{{"method":"{method}"}}"#))
    }

    fn query(
        &mut self,
        project: WitProjectId,
        _method: String,
        _params: String,
    ) -> Result<String, WitError> {
        self.check(&project)?;
        Ok(r#"{"revision":3}"#.to_owned())
    }

    fn open_projects(&mut self) -> Vec<WitProjectId> {
        vec![self.project.id.into()]
    }

    fn project_info(&mut self, project: WitProjectId) -> Result<ProjectMetadata, WitError> {
        self.check(&project)?;
        Ok(project_metadata(&self.project, self.revision))
    }

    fn sequences(&mut self, project: WitProjectId) -> Result<Vec<SequenceMetadata>, WitError> {
        self.check(&project)?;
        Ok(self
            .project
            .sequences
            .iter()
            .map(sequence_metadata)
            .collect())
    }

    fn tracks(
        &mut self,
        project: WitProjectId,
        sequence: WitSequenceId,
    ) -> Result<Vec<TrackMetadata>, WitError> {
        self.check(&project)?;
        let sequence = self.sequence(&sequence)?;
        let rate = sequence.settings.frame_rate;
        Ok(sequence
            .tracks
            .iter()
            .map(|track| track_metadata(track, rate))
            .collect())
    }

    fn clips(
        &mut self,
        project: WitProjectId,
        sequence: WitSequenceId,
        track: WitTrackId,
    ) -> Result<Vec<ClipMetadata>, WitError> {
        self.check(&project)?;
        let sequence = self.sequence(&sequence)?;
        let id = sub_model::TrackId::try_from(&track).map_err(WitError::from)?;
        let track = sequence.track(id).ok_or_else(|| {
            WitError::from(SubError::new(
                ErrorCode::from_static("plugin.no_such_track"),
                "no such track",
            ))
        })?;
        Ok(track_clip_metadata(track, sequence.settings.frame_rate))
    }

    fn markers(
        &mut self,
        project: WitProjectId,
        sequence: WitSequenceId,
    ) -> Result<Vec<MarkerMetadata>, WitError> {
        self.check(&project)?;
        Ok(self
            .sequence(&sequence)?
            .markers
            .iter()
            .map(marker_metadata)
            .collect())
    }

    fn playhead(&mut self, project: WitProjectId) -> Result<sub_plugin::WitRationalTime, WitError> {
        self.check(&project)?;
        Ok(self.playhead.into())
    }

    fn log(&mut self, level: LogLevel, message: String) {
        self.log.push((level, message));
    }
}

#[test]
fn the_accessors_answer_out_of_the_model() {
    let mut host = TestHost::new();
    let project: WitProjectId = host.project.id.into();

    let info = host.project_info(project.clone()).unwrap();
    assert_eq!(info.name, "Fixture");
    assert_eq!(info.revision, 3);
    assert_eq!(info.media_count, 1);

    let sequences = host.sequences(project.clone()).unwrap();
    assert_eq!(sequences.len(), 1);
    let sequence = sequences[0].id.clone();
    assert_eq!(sequences[0].frame_rate.numerator, 24_000);
    assert_eq!(sequences[0].frame_rate.denominator, 1_001);

    let tracks = host.tracks(project.clone(), sequence.clone()).unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].clip_count, 1);

    let clips = host
        .clips(project.clone(), sequence.clone(), tracks[0].id.clone())
        .unwrap();
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].name, "A");
    assert_eq!(
        TimeRange::try_from(clips[0].timeline_range).unwrap(),
        TimeRange::new(
            RationalTime::zero(fps()),
            RationalTime::from_frames(24, fps())
        )
        .unwrap()
    );

    let markers = host.markers(project.clone(), sequence).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].name, "sync");

    // The playhead crosses as an exact fraction and comes back unchanged.
    let playhead = host.playhead(project.clone()).unwrap();
    assert_eq!(RationalTime::try_from(playhead).unwrap(), host.playhead);

    assert_eq!(host.open_projects(), vec![project.clone()]);
    host.log(LogLevel::Info, "hello".to_owned());
    assert_eq!(host.log.len(), 1);
    assert!(
        host.run_command(project, "sequence.add_marker".to_owned(), "{}".to_owned())
            .is_ok()
    );
}

#[test]
fn a_call_naming_another_project_is_refused_with_a_stable_code() {
    let mut host = TestHost::new();
    let other = WitProjectId::from(ProjectId::new());
    let err = host.project_info(other.clone()).unwrap_err();
    assert_eq!(err.code, "plugin.no_such_project");

    let err = host
        .query(
            WitProjectId {
                value: "not-a-uuid".to_owned(),
            },
            "project.get".to_owned(),
            "{}".to_owned(),
        )
        .unwrap_err();
    assert_eq!(err.code, "model.invalid_id");
}

#[test]
fn the_command_world_links_with_every_import_satisfied() {
    let engine = wasmtime::Engine::default();
    let mut linker = wasmtime::component::Linker::<TestHost>::new(&engine);
    Command::add_to_linker::<_, wasmtime::component::HasSelf<TestHost>>(&mut linker, |state| state)
        .expect("the command world's imports are all implemented");
}

#[test]
fn the_commands_world_links_against_the_same_host() {
    // The richer world with menu and shortcut registration imports the same
    // `command-api`, so a host serving one serves both: the two worlds share
    // their Rust types rather than each generating their own.
    let engine = wasmtime::Engine::default();
    let mut linker = wasmtime::component::Linker::<TestHost>::new(&engine);
    Commands::add_to_linker::<_, wasmtime::component::HasSelf<TestHost>>(&mut linker, |state| {
        state
    })
    .expect("the commands world's imports are all implemented");
}
