//! Plugin SDK for Subordinate.
//!
//! Wraps the generated WIT guest bindings with ergonomic types and helpers so
//! a plugin author (human or agent) never touches raw component boilerplate.
//! Licensed MIT OR Apache-2.0 so plugins may use any licence. See
//! docs/PLAN.md §6.4.
//!
//! # What a plugin sees
//!
//! [`bindings`] is the guest half of `subordinate:plugin@0.1.0`, generated from
//! the same `wit/subordinate-plugin.wit` the host generates its side from. The
//! whole host interface is one module, [`command_api`]:
//!
//! - [`command_api::run_command`] and [`command_api::query`] take a JSON-RPC
//!   method name and a `params` JSON string, exactly what the Command API
//!   socket takes. A `run-command` call is one undoable command on the host's
//!   stack; `query` cannot mutate anything.
//! - [`command_api::open_projects`], [`command_api::project_info`],
//!   [`command_api::sequences`], [`command_api::tracks`],
//!   [`command_api::clips`], [`command_api::markers`] and
//!   [`command_api::playhead`] are the read-only metadata accessors. They
//!   return records, so a plugin learns the timebase it must respect without
//!   parsing JSON.
//! - [`command_api::log`] emits one line into the host's `tracing` subscriber.
//!
//! Times cross as [`RationalTime`] with a fractional rate, never as seconds,
//! and identifiers cross as distinct records, so a [`TrackId`] cannot be passed
//! where a [`ClipId`] is expected.
//!
//! Typed helpers over these bindings — serde-shaped parameters, a project
//! handle, error constructors — arrive with the SDK proper (TASK-89). This
//! crate is currently the bindings and their re-exports.

pub mod bindings;

pub use bindings::subordinate::plugin::command_api;
pub use bindings::subordinate::plugin::command_api::{
    ClipMetadata, LogLevel, MarkerMetadata, ProjectMetadata, Resolution, SequenceMetadata,
    TrackKind, TrackMetadata,
};
pub use bindings::subordinate::plugin::types;
pub use bindings::subordinate::plugin::types::{
    ClipId, Detail, Error, MarkerId, MediaId, ProjectId, Rational, RationalTime, SequenceId,
    TimeRange, TrackId,
};
pub use bindings::{Guest, export};

#[cfg(test)]
mod tests {
    use super::{ClipId, Guest, LogLevel, ProjectId, Rational, RationalTime, TrackId};

    /// A plugin only has to implement [`Guest`]; `export!` then wires it into
    /// the component's exports. Implementing it here keeps the trait's shape
    /// under test on the host triple, where the imports are never called.
    struct Plugin;

    impl Guest for Plugin {
        fn run(project: ProjectId, args: String) -> Result<String, super::Error> {
            Ok(format!("{} {args}", project.value))
        }
    }

    #[test]
    fn the_guest_trait_takes_typed_identifiers_and_returns_json() {
        assert_eq!(
            Plugin::run(
                ProjectId {
                    value: "p".to_owned()
                },
                "{}".to_owned()
            )
            .unwrap(),
            "p {}"
        );
    }

    #[test]
    fn identifiers_are_distinct_types_and_time_carries_a_fractional_rate() {
        let clip = ClipId {
            value: "c".to_owned(),
        };
        let track = TrackId {
            value: "t".to_owned(),
        };
        // Distinct records: `clip` could not be passed where `track` is.
        assert_ne!(clip.value, track.value);

        let time = RationalTime {
            value: 24,
            rate: Rational {
                numerator: 24_000,
                denominator: 1_001,
            },
        };
        assert_eq!(time.rate.denominator, 1_001);
        assert!(matches!(LogLevel::Info, LogLevel::Info));
    }
}
