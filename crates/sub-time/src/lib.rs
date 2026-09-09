//! Rational time, timecode and frame-rate primitives.
//!
//! Every timeline position and duration in Subordinate is a [`RationalTime`]
//! (an integer count at an exact rational rate); floats are never used for
//! time. This crate also handles drop-frame and non-drop-frame timecode
//! formatting and parsing. See docs/PLAN.md §5.1.
//!
//! ```
//! use sub_time::{Rational, RationalTime, TimeRange};
//!
//! // One frame of 23.976 fps material plus one frame of 29.97 fps material,
//! // computed exactly at their common 120000/1001 rate.
//! let a = RationalTime::new(1, Rational::FPS_23_976);
//! let b = RationalTime::new(1, Rational::FPS_29_97);
//! let sum = a + b;
//! assert_eq!(sum.rate(), Rational::new(120_000, 1001).unwrap());
//! assert_eq!(sum.value(), 9);
//!
//! let clip = TimeRange::new(a, RationalTime::new(48, Rational::FPS_23_976)).unwrap();
//! assert!(clip.contains(RationalTime::new(10, Rational::FPS_23_976)));
//! ```
//!
//! ```
//! use sub_time::{Rational, Timecode, TimecodeRate};
//!
//! // Drop-frame counting skips labels so the clock tracks wall time.
//! let rate = TimecodeRate::drop_frame(Rational::FPS_29_97).unwrap();
//! assert_eq!(Timecode::from_frame_number(17_982, rate).to_string(), "00;10;00;00");
//! ```

mod rational;
mod rational_time;
mod time_range;
pub mod timecode;

pub use rational::{InvalidRational, Rational};
pub use rational_time::{RationalTime, Rounding};
pub use time_range::{InvalidTimeRange, TimeRange};
pub use timecode::{Timecode, TimecodeRate};
