//! WASM plugin host on wasmtime: manifests, capabilities, WIT worlds and hot reload.
//!
//! Plugins are sandboxed WebAssembly components with versioned WIT
//! interfaces. Fuel and epoch limits keep a misbehaving plugin from stalling
//! the engine. See docs/PLAN.md §6.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_links() {}
}
