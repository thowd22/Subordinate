//! A `subordinate:plugin/command` plugin that allocates without bound.
//!
//! It grows one buffer a megabyte at a time and never frees it, so the store's
//! memory ceiling is what stops it: `memory.grow` starts failing, the guest
//! allocator aborts, and the call comes back to the host as a trap rather than
//! as an out-of-memory host process (TASK-84).

wit_bindgen::generate!({
    path: "../../../../../wit",
    world: "command",
});

struct Plugin;

export!(Plugin);

/// One megabyte per step: small enough that the ceiling is hit before the fuel
/// budget, large enough that a run is short.
const STEP: usize = 1024 * 1024;

impl Guest for Plugin {
    fn run(_project: ProjectId, _args: String) -> Result<String, Error> {
        let mut held: Vec<Vec<u8>> = Vec::new();
        loop {
            let mut chunk = vec![0_u8; STEP];
            // Touch every page so the allocation is really backed, not just
            // reserved, and so the optimiser cannot drop it.
            for (index, byte) in chunk.iter_mut().enumerate() {
                *byte = u8::try_from(index % 251).unwrap_or(0);
            }
            held.push(std::hint::black_box(chunk));
        }
    }
}
