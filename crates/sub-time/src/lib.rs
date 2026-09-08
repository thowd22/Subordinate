//! Rational time, timecode and frame-rate primitives.
//!
//! Every timeline position and duration in Subordinate is a `RationalTime`
//! (an integer count at an exact rational rate); floats are never used for
//! time. This crate also handles drop-frame and non-drop-frame timecode
//! formatting and parsing. See docs/PLAN.md §5.1.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_links() {}
}
