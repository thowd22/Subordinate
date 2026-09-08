//! GPU compositor on wgpu: frame graph, NV12 conversion, transforms and shader effects.
//!
//! The same graph serves the preview (may drop frames) and export (never
//! drops). Plugin effects are WGSL shaders plus a parameter schema compiled
//! and cached here. See docs/PLAN.md §5.3.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_links() {}
}
