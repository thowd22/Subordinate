//! The migration registry: how a project file written by an older build
//! becomes one this build can read.
//!
//! Old files must always open (docs/PLAN.md §5.6), so the loader never parses
//! a foreign version as if its fields still meant the same thing. It parses
//! the file as plain JSON, walks a chain of [`Migration`] steps from the
//! version the file declares up to [`crate::SCHEMA_VERSION`], and only then
//! deserialises the model. Every load reports which version it started from
//! and which steps ran, so the UI, the CLI and an agent can all say "this
//! project was upgraded from version 1".
//!
//! Each step is registered against the version it reads, so the chain is a
//! lookup rather than a growing `match`: adding version *n+1* means bumping
//! [`crate::SCHEMA_VERSION`] and registering one migration from *n*.
//!
//! ```
//! use sub_model::migrate::{FnMigration, MigrationRegistry};
//!
//! // A registry that upgrades version 1 files to version 2 by renaming a field.
//! let mut registry = MigrationRegistry::empty(2);
//! registry
//!     .register(FnMigration::new(1, 2, "rename title to name", |mut file| {
//!         let project = file["project"].as_object_mut().unwrap();
//!         if let Some(title) = project.remove("title") {
//!             project.insert("name".to_owned(), title);
//!         }
//!         Ok(file)
//!     }))
//!     .unwrap();
//!
//! let old = serde_json::json!({ "schema_version": 1, "project": { "title": "Doc cut" } });
//! let (new, report) = registry.migrate(old).unwrap();
//! assert_eq!(new["project"]["name"], "Doc cut");
//! assert_eq!(new["schema_version"], 2);
//! assert_eq!(report.original_version, 1);
//! assert!(report.migrated());
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sub_core::{SubError, SubResult};

use crate::codes;
use crate::json::SCHEMA_VERSION;

/// One step of the upgrade chain: it reads project files at
/// [`from_version`](Migration::from_version) and rewrites them into the shape
/// [`to_version`](Migration::to_version) expects.
///
/// The step works on the whole file as a [`Value`] — the `schema_version` and
/// `project` members both — because a schema change may move data across that
/// boundary. It does not have to set `schema_version` itself: the registry
/// stamps the new version on after the step returns.
// `from_version` reads a field, it is not a `From`-style constructor; the pair
// `from_version`/`to_version` is the vocabulary the schema chain is discussed in.
#[allow(clippy::wrong_self_convention)]
pub trait Migration: fmt::Debug + Send + Sync {
    /// The schema version this step reads.
    fn from_version(&self) -> u32;

    /// The schema version this step produces. Always greater than
    /// [`from_version`](Migration::from_version).
    fn to_version(&self) -> u32;

    /// A short human-readable summary of what the step changes, for the load
    /// report and for logs.
    fn describe(&self) -> &str;

    /// Rewrites one project file.
    ///
    /// # Errors
    ///
    /// Returns any [`SubError`] when the file does not have the shape
    /// [`from_version`](Migration::from_version) promised and the step cannot
    /// make sense of it; the registry wraps it as `model.migration_failed`.
    fn migrate(&self, file: Value) -> SubResult<Value>;
}

/// A [`Migration`] built from a closure, which is all most steps need.
pub struct FnMigration {
    from_version: u32,
    to_version: u32,
    description: &'static str,
    function: Box<dyn Fn(Value) -> SubResult<Value> + Send + Sync>,
}

impl FnMigration {
    /// Builds a step from `from_version` to `to_version` running `function`.
    pub fn new<F>(
        from_version: u32,
        to_version: u32,
        description: &'static str,
        function: F,
    ) -> Self
    where
        F: Fn(Value) -> SubResult<Value> + Send + Sync + 'static,
    {
        Self {
            from_version,
            to_version,
            description,
            function: Box::new(function),
        }
    }
}

impl fmt::Debug for FnMigration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FnMigration")
            .field("from_version", &self.from_version)
            .field("to_version", &self.to_version)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

impl Migration for FnMigration {
    fn from_version(&self) -> u32 {
        self.from_version
    }

    fn to_version(&self) -> u32 {
        self.to_version
    }

    fn describe(&self) -> &str {
        self.description
    }

    fn migrate(&self, file: Value) -> SubResult<Value> {
        (self.function)(file)
    }
}

/// One step recorded in a [`LoadReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedMigration {
    /// The version the step read.
    pub from_version: u32,
    /// The version the step produced.
    pub to_version: u32,
    /// What the step changed, from [`Migration::describe`].
    pub description: String,
}

/// What happened while loading a project file.
///
/// The original version is worth keeping even when nothing ran: a caller that
/// saves the project back writes it at [`SCHEMA_VERSION`], so this is the only
/// record that the file on disk used to be older.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadReport {
    /// The `schema_version` the file on disk declared.
    pub original_version: u32,
    /// The version the file was migrated to, which is the registry's target.
    pub final_version: u32,
    /// The steps that ran, in the order they ran.
    pub applied: Vec<AppliedMigration>,
}

impl LoadReport {
    /// True when the file was written at an older version and was upgraded.
    #[must_use]
    pub fn migrated(&self) -> bool {
        !self.applied.is_empty()
    }
}

/// The chain of [`Migration`] steps that brings any supported older file up to
/// a target schema version.
///
/// Steps are keyed by the version they read, so at most one step ever applies
/// to a given version and the chain is walked without search.
#[derive(Debug, Clone)]
pub struct MigrationRegistry {
    target_version: u32,
    steps: BTreeMap<u32, Arc<dyn Migration>>,
}

impl MigrationRegistry {
    /// An empty registry targeting `target_version`, for tests and for tools
    /// that migrate to something other than the current schema.
    #[must_use]
    pub fn empty(target_version: u32) -> Self {
        Self {
            target_version,
            steps: BTreeMap::new(),
        }
    }

    /// The registry this build loads project files with: every migration up to
    /// [`SCHEMA_VERSION`].
    ///
    /// Version 1 is the first schema, so there is nothing to upgrade from yet.
    /// Adding version 2 means bumping [`SCHEMA_VERSION`] and registering one
    /// step from 1 to 2 here.
    #[must_use]
    pub fn current() -> Self {
        Self::empty(SCHEMA_VERSION)
    }

    /// The version this registry migrates files up to.
    #[must_use]
    pub fn target_version(&self) -> u32 {
        self.target_version
    }

    /// The registered steps, ordered by the version they read.
    pub fn migrations(&self) -> impl ExactSizeIterator<Item = &dyn Migration> {
        self.steps.values().map(AsRef::as_ref)
    }

    /// Adds a step to the chain.
    ///
    /// # Errors
    ///
    /// Returns `core.invalid_argument` when the step does not move a file
    /// forward (`to_version` not greater than `from_version`), when it would
    /// overshoot [`target_version`](Self::target_version), or when another step
    /// already reads the same version.
    pub fn register(&mut self, migration: impl Migration + 'static) -> SubResult<()> {
        let (from, to) = (migration.from_version(), migration.to_version());
        if to <= from {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "a migration must move a project file to a later schema version",
            )
            .with_detail("from_version", from)
            .with_detail("to_version", to));
        }
        if to > self.target_version {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "a migration cannot produce a schema version beyond the registry target",
            )
            .with_detail("to_version", to)
            .with_detail("target_version", self.target_version));
        }
        if self.steps.contains_key(&from) {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "another migration already reads this schema version",
            )
            .with_detail("from_version", from));
        }
        self.steps.insert(from, Arc::new(migration));
        Ok(())
    }

    /// Applies every step needed to bring `file` up to
    /// [`target_version`](Self::target_version).
    ///
    /// `file` is the whole project file, not just its `project` member. The
    /// returned value carries the target `schema_version`.
    ///
    /// # Errors
    ///
    /// - `model.invalid_project_file` when the value has no top-level
    ///   `schema_version` integer.
    /// - `model.unsupported_schema_version` when the file is newer than the
    ///   target, or when no step reads the version the chain has reached.
    /// - `model.migration_failed` when a step itself fails.
    pub fn migrate(&self, file: Value) -> SubResult<(Value, LoadReport)> {
        let original_version = schema_version_of(&file)?;
        if original_version > self.target_version {
            return Err(SubError::new(
                codes::UNSUPPORTED_SCHEMA_VERSION,
                "project file was written by a newer version of Subordinate",
            )
            .with_detail("schema_version", original_version)
            .with_detail("supported_schema_version", self.target_version));
        }

        let mut file = file;
        let mut version = original_version;
        let mut applied = Vec::new();
        while version < self.target_version {
            let step = self.steps.get(&version).ok_or_else(|| {
                SubError::new(
                    codes::UNSUPPORTED_SCHEMA_VERSION,
                    "project file uses an older schema version this build has no migration for",
                )
                .with_detail("schema_version", original_version)
                .with_detail("stuck_at_version", version)
                .with_detail("supported_schema_version", self.target_version)
            })?;

            file = step.migrate(file).map_err(|err| {
                SubError::wrap(
                    codes::MIGRATION_FAILED,
                    "a project file migration failed",
                    &err,
                )
                .with_detail("from_version", step.from_version())
                .with_detail("to_version", step.to_version())
                .with_detail("migration", step.describe())
            })?;
            stamp_version(&mut file, step.to_version())?;

            applied.push(AppliedMigration {
                from_version: step.from_version(),
                to_version: step.to_version(),
                description: step.describe().to_owned(),
            });
            version = step.to_version();
        }

        Ok((
            file,
            LoadReport {
                original_version,
                final_version: version,
                applied,
            },
        ))
    }
}

impl Default for MigrationRegistry {
    fn default() -> Self {
        Self::current()
    }
}

/// The `schema_version` a parsed project file declares.
///
/// # Errors
///
/// Returns `model.invalid_project_file` when the member is missing or is not a
/// `u32`.
pub(crate) fn schema_version_of(file: &Value) -> SubResult<u32> {
    file.get("schema_version")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| {
            SubError::new(
                codes::INVALID_PROJECT_FILE,
                "project file has no top-level schema_version integer",
            )
        })
}

/// Writes `version` into a migrated file, so a step never has to remember to.
fn stamp_version(file: &mut Value, version: u32) -> SubResult<()> {
    let object = file.as_object_mut().ok_or_else(|| {
        SubError::new(
            codes::MIGRATION_FAILED,
            "a migration turned the project file into something that is not a JSON object",
        )
        .with_detail("to_version", version)
    })?;
    object.insert("schema_version".to_owned(), Value::from(version));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file at `version` whose project carries `name` under `field`.
    fn file(version: u32, field: &str, name: &str) -> Value {
        serde_json::json!({ "schema_version": version, "project": { field: name } })
    }

    /// A step renaming one project member to another.
    fn rename(from: u32, to: u32, old: &'static str, new: &'static str) -> FnMigration {
        FnMigration::new(from, to, "rename a project member", move |mut file| {
            let project = file["project"].as_object_mut().ok_or_else(|| {
                SubError::new(codes::MIGRATION_FAILED, "project member is not an object")
            })?;
            if let Some(value) = project.remove(old) {
                project.insert(new.to_owned(), value);
            }
            Ok(file)
        })
    }

    #[test]
    fn the_current_registry_targets_the_version_this_build_writes() {
        let registry = MigrationRegistry::current();
        assert_eq!(registry.target_version(), SCHEMA_VERSION);
        assert_eq!(registry.migrations().len(), SCHEMA_VERSION as usize - 1);
        assert_eq!(
            MigrationRegistry::default().target_version(),
            SCHEMA_VERSION
        );
    }

    #[test]
    fn a_file_at_the_target_version_is_untouched() {
        let registry = MigrationRegistry::current();
        let original = file(SCHEMA_VERSION, "name", "Doc cut");
        let (migrated, report) = registry.migrate(original.clone()).unwrap();
        assert_eq!(migrated, original);
        assert_eq!(report.original_version, SCHEMA_VERSION);
        assert_eq!(report.final_version, SCHEMA_VERSION);
        assert!(!report.migrated());
        assert!(report.applied.is_empty());
    }

    #[test]
    fn steps_run_in_order_and_the_report_names_each_one() {
        let mut registry = MigrationRegistry::empty(3);
        // Registered out of order, to prove the chain is walked by version.
        registry.register(rename(2, 3, "label", "name")).unwrap();
        registry.register(rename(1, 2, "title", "label")).unwrap();

        let (migrated, report) = registry.migrate(file(1, "title", "Doc cut")).unwrap();
        assert_eq!(migrated["project"]["name"], "Doc cut");
        assert_eq!(migrated["schema_version"], 3);
        assert_eq!(report.original_version, 1);
        assert_eq!(report.final_version, 3);
        assert!(report.migrated());
        let steps: Vec<(u32, u32)> = report
            .applied
            .iter()
            .map(|step| (step.from_version, step.to_version))
            .collect();
        assert_eq!(steps, vec![(1, 2), (2, 3)]);
        assert_eq!(report.applied[0].description, "rename a project member");
    }

    #[test]
    fn a_step_may_skip_versions() {
        let mut registry = MigrationRegistry::empty(4);
        registry.register(rename(1, 4, "title", "name")).unwrap();
        let (migrated, report) = registry.migrate(file(1, "title", "Doc cut")).unwrap();
        assert_eq!(migrated["schema_version"], 4);
        assert_eq!(report.applied.len(), 1);
    }

    #[test]
    fn a_gap_in_the_chain_is_reported_as_an_unsupported_version() {
        let mut registry = MigrationRegistry::empty(3);
        registry.register(rename(2, 3, "title", "name")).unwrap();
        let err = registry.migrate(file(1, "title", "Doc cut")).unwrap_err();
        assert_eq!(err.code, codes::UNSUPPORTED_SCHEMA_VERSION);
        assert_eq!(err.details.get("schema_version"), Some(&Value::from(1_u32)));
        assert_eq!(
            err.details.get("stuck_at_version"),
            Some(&Value::from(1_u32))
        );
    }

    #[test]
    fn a_newer_file_is_refused() {
        let registry = MigrationRegistry::empty(2);
        let err = registry.migrate(file(3, "name", "Doc cut")).unwrap_err();
        assert_eq!(err.code, codes::UNSUPPORTED_SCHEMA_VERSION);
        assert!(err.message.contains("newer version"), "{}", err.message);
    }

    #[test]
    fn a_failing_step_is_wrapped_with_the_step_it_failed_in() {
        let mut registry = MigrationRegistry::empty(2);
        registry
            .register(FnMigration::new(1, 2, "always fails", |_| {
                Err(SubError::new(
                    codes::INVALID_PROJECT_FILE,
                    "nothing here to upgrade",
                ))
            }))
            .unwrap();
        let err = registry.migrate(file(1, "title", "Doc cut")).unwrap_err();
        assert_eq!(err.code, codes::MIGRATION_FAILED);
        assert_eq!(
            err.details.get("migration"),
            Some(&Value::from("always fails"))
        );
        assert!(err.cause.is_some());
    }

    #[test]
    fn a_step_that_destroys_the_file_object_is_caught() {
        let mut registry = MigrationRegistry::empty(2);
        registry
            .register(FnMigration::new(1, 2, "returns a list", |_| {
                Ok(Value::Array(Vec::new()))
            }))
            .unwrap();
        let err = registry.migrate(file(1, "title", "Doc cut")).unwrap_err();
        assert_eq!(err.code, codes::MIGRATION_FAILED);
    }

    #[test]
    fn a_file_without_a_schema_version_is_refused() {
        let err = MigrationRegistry::current()
            .migrate(serde_json::json!({ "project": {} }))
            .unwrap_err();
        assert_eq!(err.code, codes::INVALID_PROJECT_FILE);
        assert!(err.message.contains("schema_version"), "{}", err.message);
    }

    #[test]
    fn a_step_that_does_not_move_forward_is_refused() {
        let mut registry = MigrationRegistry::empty(3);
        for (from, to) in [(2_u32, 2_u32), (3, 2)] {
            let err = registry
                .register(rename(from, to, "title", "name"))
                .unwrap_err();
            assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);
            assert!(err.message.contains("later schema version"), "{err}");
        }
    }

    #[test]
    fn a_step_beyond_the_target_is_refused() {
        let mut registry = MigrationRegistry::empty(2);
        let err = registry
            .register(rename(1, 3, "title", "name"))
            .unwrap_err();
        assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);
        assert_eq!(err.details.get("target_version"), Some(&Value::from(2_u32)));
    }

    #[test]
    fn two_steps_may_not_read_the_same_version() {
        let mut registry = MigrationRegistry::empty(3);
        registry.register(rename(1, 2, "title", "name")).unwrap();
        let err = registry
            .register(rename(1, 3, "title", "name"))
            .unwrap_err();
        assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);
        assert!(err.message.contains("already reads"), "{err}");
    }
}
