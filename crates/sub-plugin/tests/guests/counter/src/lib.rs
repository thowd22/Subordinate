//! A `subordinate:plugin/command` plugin that counts its own calls.
//!
//! The count lives in instance state, so it says how many times *this*
//! instance has been entered: a fresh instance answers 1, a warm one taken
//! from the host's pool answers 2, 3, ... That is what makes instance reuse
//! observable from the host side (TASK-84).

wit_bindgen::generate!({
    path: "../../../../../wit",
    world: "command",
});

use std::cell::Cell;

struct Plugin;

export!(Plugin);

thread_local! {
    /// Calls this instance has seen. A component instance is single-threaded,
    /// so a thread-local cell is the whole of the state this needs.
    static CALLS: Cell<u64> = const { Cell::new(0) };
}

impl Guest for Plugin {
    fn run(_project: ProjectId, _args: String) -> Result<String, Error> {
        let calls = CALLS.with(|calls| {
            calls.set(calls.get() + 1);
            calls.get()
        });
        Ok(format!("{{\"calls\":{calls}}}"))
    }
}
