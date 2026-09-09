# Project file JSON Schema

`project-v<N>.schema.json` is the JSON Schema of a Subordinate project file
(`.sub`) at schema version `<N>`, generated from the Rust model with
[schemars](https://graham.cool/schemars/). It is committed so agents, plugin
authors and external tools can validate a project file without building the
app.

The file is generated, not hand-edited. `sub_model::json::SCHEMA_VERSION` is the
version this build writes; the `committed_schema_is_up_to_date` test in
`crates/sub-model/src/json.rs` fails when the committed copy drifts from the
model. Regenerate it with:

```sh
SUB_UPDATE_SCHEMA=1 cargo test -p sub-model committed_schema_is_up_to_date
```

Bumping `SCHEMA_VERSION` adds a new `project-v<N>.schema.json` beside the old
ones; earlier versions stay committed so a migration can be checked against the
shape it reads. Each bump also registers one `Migration` from the previous
version in `MigrationRegistry::current` (`crates/sub-model/src/migrate.rs`), so
an old file is upgraded before it is deserialised and never fails to open.
