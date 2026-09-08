//! Audio decode, resampling, the lock-free mixer graph and cpal output.
//!
//! The real-time audio callback never locks or allocates. The audio clock is
//! the playback master; video follows it. See docs/PLAN.md §5.4.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_links() {}
}
