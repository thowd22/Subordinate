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

# Command API JSON Schema

`command-api.json` describes every method of the Command API: its name, its
kind (`command`, `query` or `session`), a one-sentence description, and the
JSON Schema of its `params` and its `result`. The MCP bridge turns these into
tools — the description is used verbatim as the MCP tool description — and the
plugin SDK generates its bindings from the same document.

It is generated, not hand-edited. Every command's parameter schema comes from
the serde type the engine decodes and its description from that command's
`Command::DESCRIPTION`, so the document cannot describe a method the build does
not serve. Print it with:

```sh
cargo run -p subordinate-cli -- schema            # pretty
cargo run -p subordinate-cli -- schema --compact  # one line
```

The `committed_schema_is_up_to_date` test in `crates/sub-command/src/schema.rs`
fails when the committed copy drifts from the generated one, which is the check
CI runs. Regenerate it with:

```sh
SUB_UPDATE_SCHEMA=1 cargo test -p sub-command committed_schema_is_up_to_date
```

# Plugin manifest JSON Schema

`plugin-manifest.json` is the data model of a plugin's `plugin.toml`
(docs/PLAN.md §6.3): identity (reverse-DNS `id`, `name`, semver `version`, the
`api` interface version), the WIT `worlds` implemented, the `[capabilities]`
requested and the `[mcp.tools.*]` declared. TOML tables are JSON objects, so a
manifest converted to JSON validates against it — which is how a scaffolding
agent can check the file it wrote without building the host.

It is generated from the Rust manifest types in `crates/sub-plugin/src/manifest.rs`,
not hand-edited. The `committed_schema_is_up_to_date` test in
`crates/sub-plugin/src/schema.rs` fails when the committed copy drifts.
Regenerate it with:

```sh
SUB_UPDATE_SCHEMA=1 cargo test -p sub-plugin committed_schema_is_up_to_date
```

# Plugin management JSON Schema

`plugin-api.json` describes the plugin management methods of the Command API —
`plugin.list`, `plugin.enable`, `plugin.disable` and `plugin.remove` — in
exactly the shape `command-api.json` uses, so the MCP bridge turns them into
tools the same way. They are separate documents because the methods are served
by the plugin host rather than by the engine: a build with no plugin directory
open still serves the whole Command API.

It is generated from the Rust registry types in
`crates/sub-plugin/src/registry.rs`, not hand-edited. The
`committed_schema_is_up_to_date` test in that module's `schema` submodule fails
when the committed copy drifts. Regenerate it with:

```sh
SUB_UPDATE_SCHEMA=1 cargo test -p sub-plugin committed_schema_is_up_to_date
```
