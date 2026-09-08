//! Project data model: projects, sequences, tracks, clips, media items and bins.
//!
//! The schema mirrors OpenTimelineIO's Timeline/Stack/Track/Clip/Gap/
//! Transition/Marker shape so a future OTIO export is trivial. Files are
//! deterministic JSON carrying a `schema_version` plus a migration registry
//! so old projects always open. See docs/PLAN.md §5.1 and §5.6.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_links() {}
}
