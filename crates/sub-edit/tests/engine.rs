//! The engine thread driven the way the Command API, the UI and the MCP
//! bridge will drive it: several client threads submitting commands at once,
//! readers taking snapshots while they do, and one subscriber counting every
//! change that comes out (docs/PLAN.md §4).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use sub_edit::commands::{AddTrack, RenameTrack};
use sub_edit::{ChangeEvent, ChangeOrigin, ChangeType, Engine, EngineConfig, EntityKind, codes};
use sub_model::{Project, Sequence, SequenceId, SequenceSettings, TrackId, TrackKind};

/// How many commands the stress test applies, and from how many threads.
const COMMANDS: usize = 10_000;
const THREADS: usize = 3;

/// A project with one sequence holding one track, which every stress command
/// renames: the project stays small, so what the test measures is the engine's
/// serialisation rather than the cost of cloning a large model.
fn fixture() -> (Project, SequenceId) {
    let mut project = Project::new("Doc cut");
    let sequence = Sequence::new("Main", SequenceSettings::default());
    let id = sequence.id;
    project.sequences.push(sequence);
    (project, id)
}

fn add_track(sequence: SequenceId, name: &str) -> AddTrack {
    AddTrack {
        sequence,
        name: name.to_owned(),
        kind: TrackKind::Video,
        index: None,
    }
}

#[test]
fn ten_thousand_commands_from_three_threads_lose_no_events() {
    let (project, sequence) = fixture();
    // The subscriber's queue is drained by its own thread, but it is also deep
    // enough to hold every event, so a lost event is a real fault rather than
    // this test merely being slow.
    let engine = Engine::with_config(
        project,
        EngineConfig {
            event_capacity: COMMANDS + 16,
            ..EngineConfig::default()
        },
    )
    .unwrap();
    engine.handle().apply(add_track(sequence, "V1")).unwrap();
    let track: TrackId = engine.handle().snapshot().sequences[0].tracks[0].id;

    let events = engine.handle().subscribe();
    let collector = thread::spawn(move || {
        let mut collected = Vec::with_capacity(COMMANDS);
        while let Some(event) = events.recv() {
            collected.push(event);
        }
        (collected, events.take_lagged())
    });

    // A reader takes snapshots throughout, to prove reading never blocks the
    // writers and never sees a half-applied project.
    let reading = Arc::new(AtomicBool::new(true));
    let reader = {
        let handle = engine.handle().clone();
        let reading = Arc::clone(&reading);
        thread::spawn(move || {
            let mut seen = 0_u64;
            while reading.load(Ordering::Relaxed) {
                let snapshot = handle.snapshot();
                assert_eq!(snapshot.sequences[0].tracks.len(), 1);
                assert!(handle.revision() >= seen);
                seen = handle.revision();
                thread::yield_now();
            }
            seen
        })
    };

    let per_thread = COMMANDS / THREADS;
    let extra = COMMANDS - per_thread * THREADS;
    let writers: Vec<_> = (0..THREADS)
        .map(|index| {
            let handle = engine.handle().clone();
            let count = per_thread + usize::from(index == 0) * extra;
            thread::spawn(move || {
                for step in 0..count {
                    handle
                        .apply(RenameTrack {
                            sequence,
                            track,
                            name: format!("V{index}-{step}"),
                        })
                        .unwrap();
                }
            })
        })
        .collect();

    for writer in writers {
        writer.join().unwrap();
    }
    reading.store(false, Ordering::Relaxed);
    reader.join().unwrap();

    // One add plus every rename.
    let total = COMMANDS as u64 + 1;
    assert_eq!(engine.handle().revision(), total);
    engine.shutdown().unwrap();

    let (collected, lagged) = collector.join().unwrap();
    assert_eq!(lagged, 0, "no subscriber may lose an event");
    assert_eq!(collected.len(), COMMANDS, "every command emits one event");

    let revisions: BTreeSet<u64> = collected.iter().map(|event| event.revision).collect();
    assert_eq!(revisions.len(), COMMANDS, "revisions are unique");
    assert_eq!(revisions.iter().next_back().copied(), Some(total));
    assert!(
        collected
            .windows(2)
            .all(|pair| pair[0].revision < pair[1].revision),
        "events arrive in revision order"
    );
    assert!(collected.iter().all(|event: &ChangeEvent| {
        event.entity == EntityKind::TRACK
            && event.change == ChangeType::Modified
            && event.origin == ChangeOrigin::Apply
            && event.id.as_deref() == Some(track.to_string().as_str())
    }));
}

#[test]
fn commands_from_several_threads_are_serialised_and_all_undoable() {
    let (project, sequence) = fixture();
    let engine = Engine::spawn(project).unwrap();

    let writers: Vec<_> = (0..THREADS)
        .map(|index| {
            let handle = engine.handle().clone();
            thread::spawn(move || {
                for step in 0..20 {
                    handle
                        .apply(add_track(sequence, &format!("V{index}-{step}")))
                        .unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }

    assert_eq!(engine.handle().snapshot().sequences[0].tracks.len(), 60);
    assert_eq!(engine.handle().history().unwrap().undo_len, 60);

    for _ in 0..60 {
        assert!(engine.handle().undo().unwrap().is_some());
    }
    assert!(engine.handle().snapshot().sequences[0].tracks.is_empty());
    assert!(engine.handle().undo().unwrap().is_none());
}

#[test]
fn a_slow_subscriber_is_told_what_it_missed_and_the_engine_runs_on() {
    let (project, sequence) = fixture();
    let engine = Engine::with_config(
        project,
        EngineConfig {
            event_capacity: 4,
            ..EngineConfig::default()
        },
    )
    .unwrap();
    let events = engine.handle().subscribe();

    for step in 0..20 {
        engine
            .handle()
            .apply(add_track(sequence, &format!("V{step}")))
            .unwrap();
    }

    assert_eq!(engine.handle().snapshot().sequences[0].tracks.len(), 20);
    let mut drained = 0;
    while events.recv_timeout(Duration::from_millis(1)).is_some() {
        drained += 1;
    }
    assert_eq!(drained, 4);
    assert_eq!(events.take_lagged(), 16);
}

#[test]
fn dropping_the_engine_stops_the_thread_and_the_handles() {
    let (project, sequence) = fixture();
    let engine = Engine::spawn(project).unwrap();
    let handle = engine.handle().clone();
    let events = handle.subscribe();
    handle.apply(add_track(sequence, "V1")).unwrap();

    drop(engine);

    assert_eq!(events.recv().unwrap().change, ChangeType::Added);
    assert_eq!(events.recv(), None);
    let err = handle.apply(add_track(sequence, "V2")).unwrap_err();
    assert_eq!(err.code, codes::ENGINE_STOPPED);
    // The last snapshot stays readable after the engine is gone.
    assert_eq!(handle.snapshot().sequences[0].tracks.len(), 1);
}
