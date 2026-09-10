//! A `subordinate:plugin/command` plugin that edits the project it is given.
//!
//! The counter guest proves a plugin runs; this one proves a plugin *edits*,
//! which is what the test harness (TASK-91) asserts project state over. It
//! reaches the host exactly as a real plugin does — `sequences` to find the
//! sequence, `run-command` to apply `track.add` — so the track it adds arrives
//! as an ordinary undoable command on the host's stack and nothing else.

wit_bindgen::generate!({
    path: "../../../../../wit",
    world: "command",
});

// The world `use`s `error` and `project-id`, so those two are already at the
// crate root; everything else is reached through the interface module.
use crate::subordinate::plugin::command_api;

struct Plugin;

export!(Plugin);

/// The track every run adds.
const TRACK: &str = "V2";

impl Guest for Plugin {
    fn run(project: ProjectId, args: String) -> Result<String, Error> {
        command_api::log(command_api::LogLevel::Info, "editor guest starting");
        let sequences = command_api::sequences(&project)?;
        let Some(sequence) = sequences.first() else {
            return Err(Error {
                code: "plugin.no_such_sequence".to_owned(),
                message: "the fixture project holds no sequence".to_owned(),
                details: Vec::new(),
            });
        };
        let params = format!(
            r#"{{"sequence":"{}","name":"{TRACK}","kind":"video"}}"#,
            sequence.id.value,
        );
        let applied = command_api::run_command(&project, "track.add", &params)?;
        Ok(format!(
            r#"{{"track":"{TRACK}","args_bytes":{},"applied_bytes":{}}}"#,
            args.len(),
            applied.len(),
        ))
    }
}
