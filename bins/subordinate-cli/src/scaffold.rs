//! `plugin new`: one command that produces a plugin an agent can build.
//!
//! The agent-first principle (docs/PLAN.md §6.1, §6.4) is that a plugin starts
//! from a single command and needs no editing before it builds, installs and
//! runs. So this writes a whole crate: a `Cargo.toml` targeting
//! `wasm32-wasip2` with the SDK feature for the chosen world, a `src/lib.rs`
//! that already implements that world's `Guest` trait, the `plugin.toml` the
//! host reads before any of the code runs, a `CLAUDE.md` describing the world
//! interface, the host imports and the test contract, and a fixture project
//! for `plugin test` to run against.
//!
//! Four worlds have templates: `command`, `effect`, `analyzer` and
//! `mcp-tools`. Naming any other world is a structured error listing the four,
//! rather than a half-written crate.
//!
//! The generated manifest is parsed with [`Manifest::parse`] before it reaches
//! the disk, so a scaffold can never leave behind a `plugin.toml` this host
//! would refuse. The report is JSON like the rest of the CLI: every file
//! written, the identity the plugin was given, and the three commands that
//! build, install and test it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sub_core::{SubError, SubResult, codes};
use sub_plugin::manifest::{Manifest, PluginId, World};

/// The worlds `plugin new` can scaffold, in the order the usage lists them.
pub const TEMPLATED_WORLDS: [World; 4] = [
    World::Command,
    World::Effect,
    World::Analyzer,
    World::McpTools,
];

/// The plugin id a scaffold gets when `--id` names none.
const DEFAULT_ID_PREFIX: &str = "com.example.";

/// The fixture project every scaffold carries, relative to the crate root.
const FIXTURE_PATH: &str = "fixture/fixture.sub";

/// The SDK version a scaffold depends on when no local checkout is named.
const SDK_VERSION: &str = "0.1";

/// What `plugin new` was asked to generate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The WIT world the plugin implements.
    pub world: World,
    /// The plugin's name, which is also its crate and directory name.
    pub name: String,
    /// Where the crate directory goes. The working directory by default.
    pub parent: Option<PathBuf>,
    /// The reverse-DNS plugin id, defaulting to `com.example.<name>`.
    pub id: Option<String>,
    /// A local `subordinate-sdk` checkout to depend on by path, instead of the
    /// published version.
    pub sdk_path: Option<PathBuf>,
    /// Whether an existing directory may be written into.
    pub force: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            world: World::Command,
            name: String::new(),
            parent: None,
            id: None,
            sdk_path: None,
            force: false,
        }
    }
}

/// Parses a world name for `--world`, listing the templated ones when it is
/// not one of them.
///
/// # Errors
///
/// `core.invalid_argument` when `world` has no template.
pub fn parse_world(world: &str) -> SubResult<World> {
    TEMPLATED_WORLDS
        .into_iter()
        .find(|candidate| candidate.as_str() == world)
        .ok_or_else(|| {
            SubError::new(
                codes::INVALID_ARGUMENT,
                format!(
                    "no template exists for the {world} world; use one of {}",
                    templated_world_list(),
                ),
            )
            .with_detail("world", world)
            .with_detail("worlds", templated_world_list())
        })
}

/// The templated worlds as the usage and the errors spell them.
fn templated_world_list() -> String {
    TEMPLATED_WORLDS
        .iter()
        .map(|world| world.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Generates a plugin crate and reports every file it wrote.
///
/// # Errors
///
/// `core.invalid_argument` for a name that is not a crate name, an id that is
/// not reverse-DNS, or a directory that already exists without `--force`;
/// `core.io` when a file cannot be written; and whatever the manifest parser
/// returns if a generated `plugin.toml` were ever invalid — which is a bug
/// here rather than something a caller can fix.
pub fn new(options: &Options) -> SubResult<Value> {
    let crate_name = crate_name(&options.name)?;
    let id = plugin_id(options, &crate_name)?;
    let display = display_name(&crate_name);
    let world = options.world;

    let root = options
        .parent
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(&crate_name);
    if root.exists() && !options.force {
        return Err(SubError::new(
            codes::INVALID_ARGUMENT,
            "refusing to write into an existing directory; pass --force to scaffold into it",
        )
        .with_detail("path", root.display().to_string()));
    }

    let manifest = manifest(&id, &display, world, &crate_name);
    // The scaffold is the one manifest nobody proof-reads, so it is validated
    // before it is written rather than at the install that comes after.
    Manifest::parse(&manifest)?;

    let mut written = Vec::new();
    write(
        &root,
        "Cargo.toml",
        &cargo_toml(&crate_name, &display, world, options.sdk_path.as_deref()),
        &mut written,
    )?;
    write(&root, "plugin.toml", &manifest, &mut written)?;
    write(
        &root,
        "CLAUDE.md",
        &claude_md(&id, &display, world, &crate_name),
        &mut written,
    )?;
    write(&root, ".gitignore", GITIGNORE, &mut written)?;
    write(
        &root,
        "src/lib.rs",
        &lib_rs(&id, &display, world, &crate_name),
        &mut written,
    )?;
    if world == World::Effect {
        write(
            &root,
            "src/effect.wgsl",
            &effect_wgsl(&display),
            &mut written,
        )?;
    }
    if world == World::McpTools {
        write(
            &root,
            &tool_schema_path(&crate_name),
            TOOL_SCHEMA,
            &mut written,
        )?;
    }

    let fixture = root.join(FIXTURE_PATH);
    create_parent(&fixture)?;
    let fixture_report = crate::project::new(&fixture, Some(&display), options.force)?;
    written.push(FIXTURE_PATH.to_owned());

    Ok(json!({
        "path": root.display().to_string(),
        "id": id.as_str(),
        "name": display,
        "crate": crate_name,
        "world": world.as_str(),
        "files": written,
        "fixture": fixture_report,
        "next_steps": next_steps(&id, &crate_name),
    }))
}

/// The three commands the scaffold is meant to be followed by
/// (docs/PLAN.md §6.4).
fn next_steps(id: &PluginId, crate_name: &str) -> Vec<String> {
    // cargo names the artefact after the library, so the hyphens in a crate
    // name are underscores in the `.wasm` the install is pointed at.
    let lib_name = crate_name.replace('-', "_");
    vec![
        "cargo build --release --target wasm32-wasip2".to_owned(),
        format!(
            "subordinate-cli plugin install ./target/wasm32-wasip2/release/{lib_name}.wasm --dev",
        ),
        format!("subordinate-cli plugin test {id}"),
    ]
}

/// Writes one generated file, creating the directories above it, and records
/// its path relative to the crate root.
fn write(root: &Path, relative: &str, contents: &str, written: &mut Vec<String>) -> SubResult<()> {
    let path = root.join(relative);
    create_parent(&path)?;
    std::fs::write(&path, contents).map_err(|err| {
        SubError::wrap(codes::IO, "a scaffolded file could not be written", &err)
            .with_detail("path", path.display().to_string())
    })?;
    written.push(relative.to_owned());
    Ok(())
}

/// Creates the directory `path` lives in.
fn create_parent(path: &Path) -> SubResult<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).map_err(|err| {
        SubError::wrap(
            codes::IO,
            "a scaffolded directory could not be created",
            &err,
        )
        .with_detail("path", parent.display().to_string())
    })
}

/// The crate, directory and id segment a `--name` becomes.
///
/// A plugin id segment and a cargo package name accept the same shape, so one
/// check covers both: lowercase letters, digits and single hyphens, starting
/// with a letter.
fn crate_name(name: &str) -> SubResult<String> {
    let candidate = name.trim().to_ascii_lowercase().replace('_', "-");
    let mut bytes = candidate.bytes();
    let valid = bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && !candidate.ends_with('-')
        && !candidate.contains("--")
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid {
        return Err(SubError::new(
            codes::INVALID_ARGUMENT,
            "a plugin name is lowercase letters, digits and hyphens, starting with a letter",
        )
        .with_detail("name", name));
    }
    Ok(candidate)
}

/// The reverse-DNS id the plugin is installed under.
fn plugin_id(options: &Options, crate_name: &str) -> SubResult<PluginId> {
    let id = options
        .id
        .clone()
        .unwrap_or_else(|| format!("{DEFAULT_ID_PREFIX}{crate_name}"));
    PluginId::parse(id.clone()).map_err(|_| {
        SubError::new(
            sub_plugin::codes::INVALID_PLUGIN_ID,
            "a plugin id is two or more dot-separated lowercase segments",
        )
        .with_detail("id", id)
    })
}

/// The display name a crate name becomes: `cut-silence` reads `Cut Silence`.
fn display_name(crate_name: &str) -> String {
    crate_name
        .split('-')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The one MCP tool an `mcp-tools` scaffold declares.
fn tool_name(crate_name: &str) -> String {
    crate_name.replace('-', "_")
}

/// Where that tool's argument schema is written.
fn tool_schema_path(crate_name: &str) -> String {
    format!("schemas/{}.json", tool_name(crate_name))
}

/// The SDK feature the chosen world is built with.
fn sdk_feature(world: World) -> &'static str {
    world.as_str()
}

/// The generated `plugin.toml`.
fn manifest(id: &PluginId, display: &str, world: World, crate_name: &str) -> String {
    let mut text = format!(
        "[plugin]\n\
         id = \"{id}\"\n\
         name = \"{display}\"\n\
         version = \"0.1.0\"\n\
         api = \"0.1\"\n\
         worlds = [\"{world}\"]\n\
         description = \"A Subordinate {world} plugin.\"\n",
    );
    if world == World::Effect {
        // The host compiles and runs a plugin's WGSL only where the shader
        // capability was granted, so an effect asks for it up front.
        text.push_str("\n[capabilities]\nshaders = true\n");
    }
    if world == World::McpTools {
        let tool = tool_name(crate_name);
        let _ = write!(
            text,
            "\n[mcp.tools.{tool}]\n\
             description = \"Reports what {display} found in the open project.\"\n\
             schema = \"{}\"\n",
            tool_schema_path(crate_name),
        );
    }
    text
}

/// The generated `Cargo.toml`.
fn cargo_toml(crate_name: &str, display: &str, world: World, sdk_path: Option<&Path>) -> String {
    let feature = sdk_feature(world);
    let sdk = sdk_path.map_or_else(
        || format!("subordinate-sdk = {{ version = \"{SDK_VERSION}\", default-features = false, features = [\"{feature}\"] }}"),
        |path| {
            format!(
                "subordinate-sdk = {{ path = \"{}\", default-features = false, features = [\"{feature}\"] }}",
                path.display().to_string().replace('\\', "\\\\"),
            )
        },
    );
    format!(
        "[package]\n\
         name = \"{crate_name}\"\n\
         description = \"{display}: a Subordinate {world} plugin\"\n\
         version = \"0.1.0\"\n\
         edition = \"2024\"\n\
         # The SDK is built with the same toolchain the host is.\n\
         rust-version = \"1.95\"\n\
         publish = false\n\
         \n\
         # A plugin is one WASM component, so it is a cdylib and nothing else.\n\
         [lib]\n\
         crate-type = [\"cdylib\"]\n\
         \n\
         [dependencies]\n\
         {sdk}\n\
         \n\
         [profile.release]\n\
         opt-level = \"s\"\n\
         strip = true\n\
         \n\
         # The crate targets wasm32-wasip2, so it keeps a workspace of its own\n\
         # rather than joining whatever workspace it was scaffolded inside.\n\
         [workspace]\n",
    )
}

/// What a scaffold never wants committed.
const GITIGNORE: &str = "/target\n";

/// The argument schema the `mcp-tools` scaffold ships.
const TOOL_SCHEMA: &str =
    "{\n  \"type\": \"object\",\n  \"properties\": {},\n  \"additionalProperties\": false\n}\n";

/// The generated `src/lib.rs` for one world.
fn lib_rs(id: &PluginId, display: &str, world: World, crate_name: &str) -> String {
    match world {
        World::Command => command_lib_rs(id, display),
        World::Effect => effect_lib_rs(display),
        World::Analyzer => analyzer_lib_rs(id, display),
        World::McpTools => mcp_lib_rs(display, crate_name),
        // Unreachable: `parse_world` accepts only the templated worlds.
        other => format!("compile_error!(\"no template for the {other} world\");\n"),
    }
}

/// The `command` world template.
fn command_lib_rs(id: &PluginId, display: &str) -> String {
    format!(
        "//! `{display}`: the `command` world of `subordinate:plugin@0.1.0`.\n\
         //!\n\
         //! `run` is handed the project to act on and a JSON object of arguments,\n\
         //! and answers with a JSON object. Every edit goes through the Command API\n\
         //! on `Project`, so a plugin edit is an ordinary undoable command that the\n\
         //! GUI, the CLI and the MCP bridge all see identically.\n\
         \n\
         use subordinate_sdk::{{Guest, Project, ProjectId, Result, export, info}};\n\
         \n\
         /// The component this crate exports.\n\
         struct Plugin;\n\
         \n\
         impl Guest for Plugin {{\n\
         \x20   fn run(project: ProjectId, args: String) -> Result<String> {{\n\
         \x20       info(\"{id} starting\");\n\
         \x20       let project = Project::new(project);\n\
         \x20       let sequences = project.sequences()?;\n\
         \n\
         \x20       // Replace this with the edit itself: build a parameter struct from\n\
         \x20       // `subordinate_sdk::params` and hand it to `project.run(&params)`.\n\
         \x20       Ok(format!(\n\
         \x20           r#\"{{{{\"sequences\":{{}},\"args_bytes\":{{}}}}}}\"#,\n\
         \x20           sequences.len(),\n\
         \x20           args.len(),\n\
         \x20       ))\n\
         \x20   }}\n\
         }}\n\
         \n\
         export!(Plugin);\n",
    )
}

/// The `effect` world template.
fn effect_lib_rs(display: &str) -> String {
    format!(
        "//! `{display}`: the `effect` world of `subordinate:plugin@0.1.0`.\n\
         //!\n\
         //! An effect declaration is static: `describe` is called once when the host\n\
         //! loads the plugin and again after a hot reload, and the host owns every\n\
         //! GPU binding from then on. Per-clip parameter values live in the project\n\
         //! model and are edited by ordinary undoable commands, never by the plugin.\n\
         \n\
         use subordinate_sdk::bindings::EffectDesc;\n\
         use subordinate_sdk::bindings::subordinate::plugin::effect_types::{{\n\
         \x20   FloatParam, ParamDesc, ParamKind,\n\
         }};\n\
         use subordinate_sdk::{{Guest, export}};\n\
         \n\
         /// The component this crate exports.\n\
         struct Plugin;\n\
         \n\
         impl Guest for Plugin {{\n\
         \x20   fn describe() -> EffectDesc {{\n\
         \x20       EffectDesc {{\n\
         \x20           params: vec![ParamDesc {{\n\
         \x20               id: \"amount\".to_owned(),\n\
         \x20               label: \"Amount\".to_owned(),\n\
         \x20               doc: \"How much of the effect to apply.\".to_owned(),\n\
         \x20               kind: ParamKind::Float(FloatParam {{\n\
         \x20                   min: 0.0,\n\
         \x20                   max: 1.0,\n\
         \x20                   default: 1.0,\n\
         \x20                   step: None,\n\
         \x20               }}),\n\
         \x20           }}],\n\
         \x20           shader: include_str!(\"effect.wgsl\").to_owned(),\n\
         \x20           entry: \"fs_main\".to_owned(),\n\
         \x20       }}\n\
         \x20   }}\n\
         }}\n\
         \n\
         export!(Plugin);\n",
    )
}

/// The `analyzer` world template.
fn analyzer_lib_rs(id: &PluginId, display: &str) -> String {
    format!(
        "//! `{display}`: the `analyzer` world of `subordinate:plugin@0.1.0`.\n\
         //!\n\
         //! An analyzer never edits the project: it studies one media item and\n\
         //! reports markers, ranges and metadata, and the host decides what to do\n\
         //! with them. The whole run is one cancellable host job, so it reports\n\
         //! progress as it goes and returns promptly once `is_cancelled` is true.\n\
         \n\
         use subordinate_sdk::bindings::subordinate::plugin::analysis_host;\n\
         use subordinate_sdk::bindings::AnalysisResult;\n\
         use subordinate_sdk::{{Detail, Guest, MediaId, Result, export, info}};\n\
         \n\
         /// How many units of work one run reports progress over.\n\
         const STEPS: u64 = 8;\n\
         \n\
         /// The component this crate exports.\n\
         struct Plugin;\n\
         \n\
         impl Guest for Plugin {{\n\
         \x20   fn analyze(media: MediaId, options: String) -> Result<AnalysisResult> {{\n\
         \x20       info(\"{id} analysing\");\n\
         \x20       for step in 0..STEPS {{\n\
         \x20           if analysis_host::is_cancelled() {{\n\
         \x20               break;\n\
         \x20           }}\n\
         \x20           // Replace this with a unit of the analysis itself.\n\
         \x20           analysis_host::report_progress(step + 1, STEPS);\n\
         \x20       }}\n\
         \x20       Ok(AnalysisResult {{\n\
         \x20           markers: Vec::new(),\n\
         \x20           ranges: Vec::new(),\n\
         \x20           metadata: vec![\n\
         \x20               Detail {{\n\
         \x20                   key: \"media\".to_owned(),\n\
         \x20                   value: format!(\"{{:?}}\", media.value),\n\
         \x20               }},\n\
         \x20               Detail {{\n\
         \x20                   key: \"options_bytes\".to_owned(),\n\
         \x20                   value: options.len().to_string(),\n\
         \x20               }},\n\
         \x20           ],\n\
         \x20       }})\n\
         \x20   }}\n\
         }}\n\
         \n\
         export!(Plugin);\n",
    )
}

/// The `mcp-tools` world template.
fn mcp_lib_rs(display: &str, crate_name: &str) -> String {
    format!(
        "//! `{display}`: the `mcp-tools` world of `subordinate:plugin@0.1.0`.\n\
         //!\n\
         //! The host prefixes every tool name with the plugin id, checks it against\n\
         //! `plugin.toml` and validates `args_json` against the tool's schema before\n\
         //! the call arrives, so `call` may trust what it is handed. A tool that\n\
         //! changes the project still goes through the Command API, so it stays one\n\
         //! undoable command like any other edit.\n\
         \n\
         use subordinate_sdk::bindings::ToolDesc;\n\
         use subordinate_sdk::{{Guest, Result, error, export, open_projects}};\n\
         \n\
         /// The tool this plugin contributes, as `plugin.toml` declares it.\n\
         const TOOL: &str = \"{tool}\";\n\
         \n\
         /// The component this crate exports.\n\
         struct Plugin;\n\
         \n\
         impl Guest for Plugin {{\n\
         \x20   fn tools() -> Vec<ToolDesc> {{\n\
         \x20       vec![ToolDesc {{\n\
         \x20           name: TOOL.to_owned(),\n\
         \x20           description: \"Reports what {display} found in the open project.\"\n\
         \x20               .to_owned(),\n\
         \x20           json_schema: include_str!(\"../schemas/{tool}.json\").to_owned(),\n\
         \x20       }}]\n\
         \x20   }}\n\
         \n\
         \x20   fn call(name: String, args_json: String) -> Result<String> {{\n\
         \x20       if name != TOOL {{\n\
         \x20           return Err(error(\"plugin.unknown_tool\", format!(\"no tool named {{name}}\")));\n\
         \x20       }}\n\
         \x20       let _ = args_json;\n\
         \x20       let projects = open_projects();\n\
         \x20       let sequences = match projects.first() {{\n\
         \x20           Some(project) => project.sequences()?.len(),\n\
         \x20           None => 0,\n\
         \x20       }};\n\
         \x20       Ok(format!(r#\"{{{{\"sequences\":{{sequences}}}}}}\"#))\n\
         \x20   }}\n\
         }}\n\
         \n\
         export!(Plugin);\n",
        tool = tool_name(crate_name),
    )
}

/// The WGSL an `effect` scaffold ships.
fn effect_wgsl(display: &str) -> String {
    format!(
        "// The fragment entry point of the `{display}` effect.\n\
         //\n\
         // The host prepends its own prelude, so this file declares none of it: the\n\
         // `EffectParams` uniform built from the parameters `describe` declares (one\n\
         // member per parameter id, so `params.amount` below), the input picture as\n\
         // `source` with `source_sampler`, and the `VsOut` the full-screen vertex\n\
         // stage hands over.\n\
         @fragment\n\
         fn fs_main(in: VsOut) -> @location(0) vec4<f32> {{\n\
         \x20   let source_colour = textureSample(source, source_sampler, in.uv);\n\
         \x20   // Replace this with the effect itself.\n\
         \x20   return vec4<f32>(source_colour.rgb * params.amount, source_colour.a);\n\
         }}\n",
    )
}

/// The generated `CLAUDE.md`: the world's interface, its host imports and the
/// test contract, written for whoever — human or agent — edits the scaffold
/// next.
fn claude_md(id: &PluginId, display: &str, world: World, crate_name: &str) -> String {
    let feature = sdk_feature(world);
    let lib_name = crate_name.replace('-', "_");
    let mut text = format!(
        "# {display}\n\n\
         A Subordinate plugin implementing the `{world}` world of \
         `subordinate:plugin@0.1.0`. It is one WASM component: one world, one \
         `Guest` implementation, `export!` to wire it up.\n\n\
         - Plugin id: `{id}` — the install directory, the log tag and the \
         capability grant all key on it.\n\
         - SDK: `subordinate-sdk`, `default-features = false, features = \
         [\"{feature}\"]`. Exactly one world feature may be on.\n\
         - Target: `wasm32-wasip2`. The Rust toolchain emits a component \
         directly, so there is no `cargo component` or `wasm-tools` step.\n\
         - Worked example: `plugins/cut-silence` in the Subordinate repository \
         is the first-party plugin this file is modelled on — an analyzer and \
         the command that acts on what it found. Its `CLAUDE.md` answers what \
         this one does not.\n\n\
         ## The interface\n\n\
         {interface}\n\
         ## Host imports\n\n\
         {imports}\n\
         Every world imports `command-api`, which is what `Project` wraps: \
         `project.run(&params)` applies one undoable command, `project.query(&params)` \
         reads without mutating, and `info`, `warn` and `error_log` write to the \
         host's log. Times cross as `RationalTime` — a tick count and an exact \
         fractional rate — never as seconds in a float, and `subordinate_sdk::time` \
         does the arithmetic without reaching for one.\n\n\
         Everything returns `Result`, whose error carries a stable `code`, a \
         one-line message and details naming the WIT item it belongs to and a hint. \
         `subordinate_sdk::errors` is the catalogue; match on a code, never on a \
         message.\n\n\
         ## Capabilities\n\n\
         {capabilities}\n\
         ## The test contract\n\n\
         `fixture/fixture.sub` is this plugin's fixture project: one sequence with \
         a video track and an audio track, written by `subordinate-cli new`. \
         `subordinate-cli plugin test {id}` loads the plugin headlessly, runs it \
         against that project and prints structured JSON; it exits non-zero when a \
         check fails.\n\n\
         {test_contract}\n\
         Edit the fixture with the CLI rather than by hand — \
         `subordinate-cli inspect fixture/fixture.sub` reports its whole structure — \
         so it stays a project this build can load.\n\n\
         ## The loop\n\n\
         ```\n\
         cargo build --release --target wasm32-wasip2\n\
         subordinate-cli plugin install ./target/wasm32-wasip2/release/{lib_name}.wasm --dev\n\
         subordinate-cli plugin test {id}\n\
         ```\n\n\
         `--dev` links the component to its sources, so a running host watches the \
         file and hot-reloads it: rebuild and the editor picks it up without a \
         restart. `subordinate-cli plugin reload {id}` forces it. Every one of these \
         is also an MCP tool, and every failure is structured JSON naming the WIT \
         item and a hint, so nothing has to be read out of a stack trace.\n\n\
         ## House rules\n\n\
         - Never hold the project model: every change is a Command API call, so it \
         lands on the host's undo stack.\n\
         - Never compute timeline positions in floating point. `RationalTime` is \
         exact; seconds are not.\n\
         - Keep `plugin.toml` truthful: the host refuses anything it declares that \
         the component does not export, and an undeclared capability is a denied \
         one.\n",
        interface = interface_section(world),
        imports = imports_section(world),
        capabilities = capabilities_section(world),
        test_contract = test_contract_section(world, crate_name),
    );
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// The `## The interface` body for one world.
fn interface_section(world: World) -> &'static str {
    match world {
        World::Command => {
            "```rust\n\
             fn run(project: ProjectId, args: String) -> Result<String>\n\
             ```\n\n\
             One entry point. `args` is a JSON object the caller supplied and the \
             answer is a JSON object of your own shape. The host opens one undo \
             group around the call, so however many primitives you apply, the user \
             undoes the whole thing in one step.\n"
        }
        World::Effect => {
            "```rust\n\
             fn describe() -> EffectDesc\n\
             ```\n\n\
             Called once at load time and again after a hot reload — never per \
             frame. `EffectDesc` carries the parameters the inspector shows, the \
             WGSL source and the name of its fragment entry point. The host derives \
             a uniform struct from the parameter list, so `src/effect.wgsl` reads \
             `params.<id>` for each one and declares neither the uniform, the input \
             texture nor the sampler itself.\n"
        }
        World::Analyzer => {
            "```rust\n\
             fn analyze(media: MediaId, options: String) -> Result<AnalysisResult>\n\
             ```\n\n\
             The host picks the media item, so this receives an identifier rather \
             than a path: a sandboxed plugin never learns where the project's media \
             lives. `options` is a JSON object of your own schema. `AnalysisResult` \
             carries markers for a person to look at, ranges for a command to act \
             on, and metadata as key/JSON pairs — all three may be empty.\n"
        }
        World::McpTools => {
            "```rust\n\
             fn tools() -> Vec<ToolDesc>\n\
             fn call(name: String, args_json: String) -> Result<String>\n\
             ```\n\n\
             `tools` is called on load and after a hot reload; the host compares what \
             it returns with `plugin.toml` and refuses any tool the manifest does not \
             declare. `name` reaches `call` without the plugin-id prefix, and \
             `args_json` has already been validated against that tool's schema.\n"
        }
        _ => "",
    }
}

/// The `## Host imports` body for one world.
fn imports_section(world: World) -> &'static str {
    match world {
        World::Analyzer => {
            "`command-api` and `analysis-host`. The second is the job channel: \
             `analysis_host::report_progress(done, total)` publishes progress — per \
             unit of work, not per sample — and `analysis_host::is_cancelled()` is \
             how cancellation reaches code inside the sandbox. An analyzer that \
             never asks cannot be stopped before it returns.\n"
        }
        _ => "`command-api`, and nothing else.\n",
    }
}

/// The `## Capabilities` body for one world.
fn capabilities_section(world: World) -> &'static str {
    match world {
        World::Effect => {
            "`plugin.toml` requests `shaders = true`, which is what lets the host \
             compile and run the WGSL. Everything else — filesystem roots, the \
             network — stays unrequested, and an unrequested capability is a denied \
             one. Add a key only when the plugin genuinely needs it: the user \
             approves the list at install time.\n"
        }
        _ => {
            "`plugin.toml` requests none, which is the right default: an \
             unrequested capability is a denied one. Add `fs_read`, `fs_write`, \
             `network` or `shaders` only when the plugin genuinely needs them, \
             because the user approves the list at install time.\n"
        }
    }
}

/// The `## The test contract` body for one world.
fn test_contract_section(world: World, crate_name: &str) -> String {
    match world {
        World::Command => {
            "A command plugin is checked by asserting project state after `run`: the \
             harness runs the plugin against the fixture and reports what the project \
             looked like afterwards, so make the effect of a run something a caller \
             can see — a clip added, a marker moved, a track renamed.\n"
                .to_owned()
        }
        World::Effect => "An effect plugin is checked by rendering a frame: the harness compiles \
             the shader, runs it over a test picture and reports the result, so a \
             shader that does not compile or an entry point that is not there fails \
             the run rather than the editor.\n"
            .to_owned(),
        World::Analyzer => "An analyzer is checked by what it returns: the harness runs `analyze` \
             over the fixture's media and reports the markers, ranges and metadata, \
             so keep the labels a small stable vocabulary — a command downstream \
             matches on them.\n"
            .to_owned(),
        World::McpTools => format!(
            "An MCP tool is checked by calling it: the harness reads `tools`, checks \
             it against `plugin.toml` and calls `{tool}` with arguments its schema \
             accepts. Keep `schemas/{tool}.json` self-contained — the host resolves \
             no remote or file references while validating.\n",
            tool = tool_name(crate_name),
        ),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use sub_plugin::manifest::{Manifest, World};

    use super::{Options, TEMPLATED_WORLDS, crate_name, display_name, new, parse_world};

    /// A scratch directory of this test's own.
    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("subordinate-cli-scaffold-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch directory");
        root
    }

    /// Options for one world under `parent`.
    fn options(world: World, name: &str, parent: &Path) -> Options {
        Options {
            world,
            name: name.to_owned(),
            parent: Some(parent.to_path_buf()),
            ..Options::default()
        }
    }

    /// The generated file at `relative` under the scaffold.
    fn read(root: &Path, relative: &str) -> String {
        std::fs::read_to_string(root.join(relative))
            .unwrap_or_else(|err| panic!("{relative} is missing: {err}"))
    }

    #[test]
    fn every_templated_world_generates_a_crate_a_manifest_a_guide_and_a_fixture() {
        let parent = scratch("worlds");
        for world in TEMPLATED_WORLDS {
            let report = new(&options(world, "demo-plugin", &parent)).expect("a scaffold");
            let root = parent.join("demo-plugin");
            assert_eq!(report["world"], world.as_str());
            assert_eq!(report["id"], "com.example.demo-plugin");
            assert_eq!(report["name"], "Demo Plugin");

            for expected in [
                "Cargo.toml",
                "plugin.toml",
                "CLAUDE.md",
                "src/lib.rs",
                "fixture/fixture.sub",
            ] {
                assert!(
                    root.join(expected).is_file(),
                    "{world}: {expected} was not written",
                );
                assert!(
                    report["files"]
                        .as_array()
                        .expect("files")
                        .iter()
                        .any(|file| file == expected),
                    "{world}: {expected} is missing from the report",
                );
            }

            // The manifest the scaffold wrote is one this host accepts, and it
            // declares the world the crate implements.
            let manifest = Manifest::read_dir(&root).expect("a valid manifest");
            assert!(manifest.declares(world), "{world}: manifest disagrees");
            assert_eq!(manifest.plugin.id.as_str(), "com.example.demo-plugin");

            // The crate depends on exactly the SDK feature for that world.
            let cargo = read(&root, "Cargo.toml");
            assert!(
                cargo.contains(&format!("features = [\"{world}\"]")),
                "{world}: {cargo}",
            );
            assert!(cargo.contains("crate-type = [\"cdylib\"]"), "{cargo}");
            assert!(cargo.contains("[workspace]"), "{cargo}");

            // The guide names the world, its interface and the test contract.
            let guide = read(&root, "CLAUDE.md");
            assert!(guide.contains(world.as_str()), "{world}: {guide}");
            assert!(guide.contains("## The interface"), "{world}");
            assert!(guide.contains("## Host imports"), "{world}");
            assert!(guide.contains("## The test contract"), "{world}");
            assert!(
                guide.contains("plugin test com.example.demo-plugin"),
                "{world}",
            );

            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn the_effect_scaffold_ships_a_shader_and_asks_for_the_shader_capability() {
        let parent = scratch("effect");
        new(&options(World::Effect, "tint", &parent)).expect("a scaffold");
        let root = parent.join("tint");
        let shader = read(&root, "src/effect.wgsl");
        assert!(shader.contains("@fragment"), "{shader}");
        assert!(shader.contains("fn fs_main"), "{shader}");
        assert!(shader.contains("params.amount"), "{shader}");
        let manifest = Manifest::read_dir(&root).expect("a valid manifest");
        assert!(manifest.capabilities.shaders);
    }

    #[test]
    fn the_mcp_scaffold_declares_one_tool_and_ships_its_schema() {
        let parent = scratch("mcp");
        new(&options(World::McpTools, "clip-count", &parent)).expect("a scaffold");
        let root = parent.join("clip-count");
        let manifest = Manifest::read_dir(&root).expect("a valid manifest");
        let tool = manifest.mcp.tools.get("clip_count").expect("one tool");
        assert_eq!(tool.schema, Path::new("schemas/clip_count.json"));
        let schema: serde_json::Value =
            serde_json::from_str(&read(&root, "schemas/clip_count.json")).expect("valid JSON");
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn the_fixture_is_a_project_the_cli_can_read_back() {
        let parent = scratch("fixture");
        let report = new(&options(World::Command, "demo", &parent)).expect("a scaffold");
        let fixture = parent.join("demo").join("fixture").join("fixture.sub");
        assert_eq!(report["fixture"]["project"]["name"], "Demo");
        let opened = crate::project::open(&fixture).expect("the fixture loads");
        assert_eq!(opened["project"]["name"], "Demo");
        let inspected = crate::project::inspect(&fixture).expect("the fixture inspects");
        assert_eq!(
            inspected["sequences"].as_array().expect("sequences").len(),
            1,
        );
    }

    #[test]
    fn a_local_sdk_checkout_is_depended_on_by_path() {
        let parent = scratch("sdk-path");
        let options = Options {
            sdk_path: Some(PathBuf::from("/checkout/sdk/subordinate-sdk")),
            ..options(World::Command, "demo", &parent)
        };
        new(&options).expect("a scaffold");
        let cargo = read(&parent.join("demo"), "Cargo.toml");
        assert!(
            cargo.contains("path = \"/checkout/sdk/subordinate-sdk\""),
            "{cargo}",
        );
        assert!(!cargo.contains("version = \"0.1\""), "{cargo}");
    }

    #[test]
    fn an_existing_directory_is_refused_unless_forced() {
        let parent = scratch("existing");
        new(&options(World::Command, "demo", &parent)).expect("a scaffold");
        let error = new(&options(World::Command, "demo", &parent)).expect_err("already there");
        assert_eq!(error.code.as_str(), "core.invalid_argument");
        let forced = Options {
            force: true,
            ..options(World::Command, "demo", &parent)
        };
        new(&forced).expect("forced over the top");
    }

    #[test]
    fn a_world_without_a_template_is_named_along_with_the_ones_that_have_one() {
        assert_eq!(parse_world("command").expect("command"), World::Command);
        assert_eq!(
            parse_world("mcp-tools").expect("mcp-tools"),
            World::McpTools
        );
        let error = parse_world("panel").expect_err("no panel template");
        assert_eq!(error.code.as_str(), "core.invalid_argument");
        assert_eq!(error.details["world"], "panel");
        assert!(
            error.details["worlds"]
                .as_str()
                .expect("the templated worlds")
                .contains("analyzer"),
        );
    }

    #[test]
    fn a_name_that_is_not_a_crate_name_is_refused_before_anything_is_written() {
        let parent = scratch("names");
        for bad in ["9lives", "my plugin", "Plugin!", "", "-lead"] {
            let error = new(&options(World::Command, bad, &parent)).expect_err("a bad name");
            assert_eq!(error.code.as_str(), "core.invalid_argument", "{bad}");
        }
        assert_eq!(
            crate_name("Cut_Silence").expect("normalised"),
            "cut-silence"
        );
    }

    #[test]
    fn an_id_that_is_not_reverse_dns_is_refused() {
        let parent = scratch("ids");
        let options = Options {
            id: Some("nodots".to_owned()),
            ..options(World::Command, "demo", &parent)
        };
        let error = new(&options).expect_err("a bad id");
        assert_eq!(error.code.as_str(), "plugin.invalid_plugin_id");
        assert_eq!(error.details["id"], "nodots");
    }

    #[test]
    fn a_display_name_is_the_crate_name_in_words() {
        assert_eq!(display_name("cut-silence"), "Cut Silence");
        assert_eq!(display_name("demo"), "Demo");
    }
}
