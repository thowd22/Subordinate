//! Loading a project file written at an older schema version.
//!
//! The committed fixture `fixtures/project-v1-renamed-field.json` is a real
//! `schema_version: 1` file whose project name is stored under `title` — the
//! kind of rename a later schema would make. The migration registry has to
//! turn it into something this build's model accepts, and the load report has
//! to say where the file came from.
//!
//! Version 1 is the schema this build writes, so there is nothing yet to
//! migrate *from* in production. The test therefore stands in a registry that
//! targets a hypothetical version 2 with the rename step a real bump would
//! register, which exercises exactly the code path
//! [`sub_model::json::from_json`] uses.

use sub_model::json;
use sub_model::migrate::{FnMigration, MigrationRegistry};

/// The fixture as it sits on disk.
fn fixture() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/project-v1-renamed-field.json"
    ))
    .expect("the schema_version 1 fixture is committed next to this test")
}

/// The step a version 2 bump would register: `project.title` became
/// `project.name`.
fn rename_title_to_name() -> FnMigration {
    FnMigration::new(1, 2, "project.title became project.name", |mut file| {
        let project = file["project"].as_object_mut().ok_or_else(|| {
            sub_core::SubError::new(
                sub_model::codes::MIGRATION_FAILED,
                "project file has no project object",
            )
        })?;
        if let Some(title) = project.remove("title") {
            project.insert("name".to_owned(), title);
        }
        Ok(file)
    })
}

#[test]
fn a_version_1_fixture_with_a_renamed_field_migrates_and_loads() {
    let mut registry = MigrationRegistry::empty(2);
    registry.register(rename_title_to_name()).unwrap();

    let (project, report) = json::from_json_with_registry(&fixture(), &registry).unwrap();

    // The renamed field arrived where the model expects it, and the rest of
    // the file survived the trip.
    assert_eq!(project.name, "Doc cut");
    assert_eq!(project.media.len(), 1);
    assert_eq!(project.sequences.len(), 1);
    assert_eq!(project.sequences[0].tracks[0].items.len(), 1);

    // The load report records where the file came from and what ran.
    assert_eq!(report.original_version, 1);
    assert_eq!(report.final_version, 2);
    assert!(report.migrated());
    assert_eq!(report.applied.len(), 1);
    assert_eq!(report.applied[0].from_version, 1);
    assert_eq!(report.applied[0].to_version, 2);
    assert_eq!(
        report.applied[0].description,
        "project.title became project.name"
    );
}

#[test]
fn the_migrated_project_saves_back_at_the_current_schema_version() {
    let mut registry = MigrationRegistry::empty(2);
    registry.register(rename_title_to_name()).unwrap();
    let (project, _) = json::from_json_with_registry(&fixture(), &registry).unwrap();

    let text = json::to_json(&project).unwrap();
    assert!(text.contains(&format!("\"schema_version\": {}", json::SCHEMA_VERSION)));
    assert!(!text.contains("\"title\""));
    assert_eq!(json::from_json(&text).unwrap(), project);
}

#[test]
fn the_fixture_does_not_load_without_its_migration() {
    // Without the rename step the file is unreadable: `title` is not a member
    // of the model, and `name` is missing.
    let err = json::from_json(&fixture()).unwrap_err();
    assert_eq!(err.code, sub_model::codes::INVALID_PROJECT_FILE);
}

#[test]
fn a_current_file_loads_with_an_empty_report() {
    let text = json::to_json(&sub_model::Project::new("Doc cut")).unwrap();
    let (project, report) = json::from_json_with_report(&text).unwrap();
    assert_eq!(project.name, "Doc cut");
    assert_eq!(report.original_version, json::SCHEMA_VERSION);
    assert!(!report.migrated());
}
