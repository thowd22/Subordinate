//! A deliberately misbehaving `subordinate:plugin/command` plugin.
//!
//! `run` never returns. The host is expected to stop it: fuel exhaustion for a
//! deterministic budget, or an epoch deadline for a wall-clock one.

wit_bindgen::generate!({
    path: "../../../../wit",
    world: "command",
});

struct Plugin;

export!(Plugin);

impl Guest for Plugin {
    fn run(_project: String, _args: String) -> Result<String, Error> {
        // `black_box` keeps the loop from being optimised into `unreachable`,
        // which would trap immediately and prove nothing about the limits.
        let mut spins: u64 = 0;
        loop {
            spins = std::hint::black_box(spins).wrapping_add(1);
        }
    }
}
