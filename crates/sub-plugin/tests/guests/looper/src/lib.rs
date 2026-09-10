//! A `subordinate:plugin/command` plugin that never returns.
//!
//! The host is expected to stop it: fuel exhaustion for a deterministic
//! budget, an epoch deadline for a wall-clock one (TASK-84).

wit_bindgen::generate!({
    path: "../../../../../wit",
    world: "command",
});

struct Plugin;

export!(Plugin);

impl Guest for Plugin {
    fn run(_project: ProjectId, _args: String) -> Result<String, Error> {
        // `black_box` keeps the loop from being optimised into `unreachable`,
        // which would trap at once and prove nothing about the limits.
        let mut spins: u64 = 0;
        loop {
            spins = std::hint::black_box(spins).wrapping_add(1);
        }
    }
}
