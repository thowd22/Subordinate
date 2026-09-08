//! The egui application: timeline, media bin, viewer, inspector and export panels.
//!
//! The UI thread never blocks on media. The viewer can pop out into its own
//! viewport for a second display. See docs/PLAN.md §5.7.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_links() {}
}
