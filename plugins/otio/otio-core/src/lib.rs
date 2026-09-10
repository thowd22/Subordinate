//! OpenTimelineIO interchange for Subordinate, minus the component glue.
//!
//! Interchange lives in plugins (docs/PLAN.md §3), and OTIO is the target the
//! model was shaped around: `Timeline`, `Stack`, `Track`, `Clip`, `Gap`,
//! `Transition` and `Marker` all have a counterpart on each side, and time is
//! rational on both. Two components ship on top of this crate:
//!
//! - `otio-importer` implements the `importer` world: it reads an `.otio` file
//!   and hands the host [`import::ImportPlan`], which the host applies through
//!   the Command API as one undoable step per item.
//! - `otio-exporter` implements the `command` world: it reads the project back
//!   through `project.get` and writes one sequence out as OTIO JSON.
//!
//! Everything that is not WIT glue is here so that both directions are
//! unit-tested on the host triple rather than only inside a sandbox.
//!
//! # What crosses, and what does not
//!
//! The cut crosses in full: tracks, clips and their source ranges, gaps,
//! crossfades, and markers on the sequence and on clips. **Effects do not** —
//! not plugin effects and not the per-clip parameters the MVP has (opacity,
//! transform, gain, fades). See [`export`] for why.
//!
//! # Time
//!
//! OTIO serialises `RationalTime` as two JSON numbers, so the exact rates the
//! model insists on have to survive a trip through an `f64`. [`time`] is the
//! only place that conversion happens, and it recognises the NTSC rates rather
//! than approximating them.

pub mod error;
pub mod export;
pub mod import;
pub mod project;
pub mod schema;
pub mod time;

pub use error::{Error, Result, codes};
pub use export::export_sequence;
pub use import::{ImportPlan, import_document, import_timeline};
pub use project::{Arguments, Project, parse_arguments, parse_project_get};
pub use schema::Timeline;
pub use time::{Rate, Span, Time};
