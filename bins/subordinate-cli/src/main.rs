//! Headless command-line interface.
//!
//! Loads, saves, inspects and renders projects without a GUI, serves the
//! Command API for out-of-process clients, and scaffolds and tests plugins.
//! It is also the CI smoke test.

use std::path::PathBuf;
use std::process::ExitCode;

use sub_plugin::registry::InstallLocation;

mod host;
mod plugin;
mod plugin_test;
mod project;
mod render;
mod scaffold;
mod serve;

/// What the arguments asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// No subcommand: print the version, as the CI smoke test expects.
    Version,
    /// Print the usage text.
    Help,
    /// Print the hardware diagnostics as JSON.
    Diag { pretty: bool },
    /// Print the JSON Schema of the Command API.
    Schema { pretty: bool },
    /// Write a new project file.
    New {
        /// Where the file goes.
        path: PathBuf,
        /// The project name, defaulting to the file stem.
        name: Option<String>,
        /// Whether an existing file may be replaced.
        force: bool,
        /// Whether the report is indented.
        pretty: bool,
    },
    /// Load a project file and report what came back.
    Open {
        /// The file to load.
        path: PathBuf,
        /// Whether the report is indented.
        pretty: bool,
    },
    /// Load a project file and write it out again, here or elsewhere.
    Save {
        /// The file to load.
        path: PathBuf,
        /// Where to write it, defaulting to `path` itself.
        output: Option<PathBuf>,
        /// Whether the report is indented.
        pretty: bool,
    },
    /// Render a sequence to a file.
    Render {
        /// The project, the sequence, the preset and where the file goes.
        options: Box<render::Options>,
        /// Whether the report is indented.
        pretty: bool,
    },
    /// Report the whole structure of a project file.
    Inspect {
        /// The file to load.
        path: PathBuf,
        /// Whether the report is indented.
        pretty: bool,
    },
    /// Serve the Command API without a GUI.
    Serve {
        /// The project, the instance and where the endpoint lives.
        options: serve::Options,
        /// Whether the readiness line and the report are indented.
        pretty: bool,
    },
    /// List installed plugins, or switch one on, off or away.
    Plugin {
        /// Which of the four operations was asked for.
        action: plugin::Action,
        /// Which plugin directories to look in.
        options: plugin::Options,
        /// Whether the report is indented.
        pretty: bool,
    },
    /// Anything unrecognised, reported with the offending argument.
    Unknown(String),
    /// A subcommand missing an argument it needs.
    Incomplete(String),
}

/// The usage text, kept next to the parser so the two cannot drift.
const USAGE: &str = "\
subordinate-cli - headless interface to Subordinate

Usage:
  subordinate-cli              print the version
  subordinate-cli diag         print hardware diagnostics as JSON
  subordinate-cli diag --compact
                               the same JSON on one line
  subordinate-cli schema       print the Command API JSON Schema
  subordinate-cli schema --compact
                               the same JSON on one line
  subordinate-cli new <file> [--name <name>] [--force]
                               write a new project file
  subordinate-cli open <file>  load a project and report its version and media
  subordinate-cli save <file> [--output <file>]
                               load a project and write it out again
  subordinate-cli inspect <file>
                               print the whole project structure as JSON
  subordinate-cli render <file> --preset <id> --out <file>
                        [--sequence <name>] [--encoder <element>]
                        [--range <in:out>] [--verify]
                               render a sequence headlessly: progress on
                               stderr, the report as JSON on stdout. The range
                               is frames of the sequence timebase, the out
                               point exclusive and either end optional;
                               --encoder pins the encoder element for the
                               preset's video codec, and --verify probes the
                               written file with the discoverer
  subordinate-cli serve [--project <file>] [--instance <name>]
                        [--directory <dir>] [--plugin-dir <dir>]
                               serve the Command API until stdin closes
  subordinate-cli plugin new --world <world> <name> [--output <dir>]
                        [--id <id>] [--sdk-path <dir>] [--force]
                               scaffold a plugin crate for one WIT world:
                               Cargo.toml, src, plugin.toml, CLAUDE.md and a
                               fixture project. Worlds: command, effect,
                               analyzer, mcp-tools
  subordinate-cli plugin install <path> [--dev] [--project-local]
                        [--dir <dir>] [--project <file>]
                               install a built .wasm (or a plugin directory);
                               --dev links it to its sources so a running host
                               watches and hot-reloads it
  subordinate-cli plugin reload <id>
                               load an installed plugin again
  subordinate-cli plugin test <id> [--fixture <file>] [--args <json>]
                        [--dir <dir>] [--project <file>]
                               load an installed plugin headlessly and run the
                               checks its declared worlds call for against a
                               fixture project: a command plugin is run and the
                               project state asserted afterwards, an effect
                               plugin has a frame rendered with it. Exits
                               non-zero when a check fails
  subordinate-cli plugin list [--dir <dir>] [--project <file>]
                               list installed plugins, load failures and
                               id conflicts
  subordinate-cli plugin enable <id> [--dir <dir>] [--project <file>]
  subordinate-cli plugin disable <id>
                               switch a plugin on or off
  subordinate-cli plugin remove <id>
                               delete an installed plugin from disk
  subordinate-cli --help       print this text

Every subcommand prints JSON, indented by default and on one line with
--compact. A failure prints a JSON SubError on stderr on the same terms: a
stable code, a message, and — for a plugin failure — the WIT type or function
it belongs to and a one-line hint saying what would fix it.
";

/// Parses the arguments after the program name.
fn parse(args: &[String]) -> Command {
    let mut args = args.iter().map(String::as_str);
    match args.next() {
        None => Command::Version,
        Some("--help" | "-h" | "help") => Command::Help,
        Some("diag") => match json_flags(args) {
            Ok(pretty) => Command::Diag { pretty },
            Err(unknown) => *unknown,
        },
        Some("schema") => match json_flags(args) {
            Ok(pretty) => Command::Schema { pretty },
            Err(unknown) => *unknown,
        },
        Some("new") => parse_new(args),
        Some("open") => parse_file(args, "open", |path, pretty| Command::Open { path, pretty }),
        Some("save") => parse_save(args),
        Some("inspect") => parse_file(args, "inspect", |path, pretty| Command::Inspect {
            path,
            pretty,
        }),
        Some("render") => parse_render(args),
        Some("serve") => parse_serve(args),
        Some("plugin") => parse_plugin(args),
        Some(other) => Command::Unknown(other.to_owned()),
    }
}

/// Reads the `--compact`, `--pretty` and `--json` flags the JSON-printing
/// subcommands share, or reports the first argument that is none of them.
///
/// The error is boxed because `Command` is a large enum: several variants
/// carry an options struct, and Windows takes the largest of them past the
/// size clippy's `result_large_err` allows in a `Result`.
fn json_flags<'a>(args: impl Iterator<Item = &'a str>) -> Result<bool, Box<Command>> {
    let mut pretty = true;
    for arg in args {
        match arg {
            "--compact" => pretty = false,
            "--pretty" | "--json" => pretty = true,
            other => return Err(Box::new(Command::Unknown(other.to_owned()))),
        }
    }
    Ok(pretty)
}

/// Reads a subcommand that takes one project file and the JSON flags.
fn parse_file<'a>(
    args: impl Iterator<Item = &'a str>,
    subcommand: &str,
    build: impl FnOnce(PathBuf, bool) -> Command,
) -> Command {
    let mut path = None;
    let mut pretty = true;
    for arg in args {
        match arg {
            "--compact" => pretty = false,
            "--pretty" | "--json" => pretty = true,
            other if path.is_none() && !other.starts_with('-') => {
                path = Some(PathBuf::from(other));
            }
            other => return Command::Unknown(other.to_owned()),
        }
    }
    path.map_or_else(
        || Command::Incomplete(format!("{subcommand} needs the path of a project file")),
        |path| build(path, pretty),
    )
}

/// Reads `new <file> [--name <name>] [--force]`.
fn parse_new<'a>(mut args: impl Iterator<Item = &'a str>) -> Command {
    let mut path = None;
    let mut name = None;
    let mut force = false;
    let mut pretty = true;
    while let Some(arg) = args.next() {
        match arg {
            "--compact" => pretty = false,
            "--pretty" | "--json" => pretty = true,
            "--force" => force = true,
            "--name" => match args.next() {
                Some(value) => name = Some(value.to_owned()),
                None => return Command::Incomplete("--name needs a project name".to_owned()),
            },
            other if path.is_none() && !other.starts_with('-') => {
                path = Some(PathBuf::from(other));
            }
            other => return Command::Unknown(other.to_owned()),
        }
    }
    path.map_or_else(
        || Command::Incomplete("new needs the path of the project file to write".to_owned()),
        |path| Command::New {
            path,
            name,
            force,
            pretty,
        },
    )
}

/// Reads `save <file> [--output <file>]`.
fn parse_save<'a>(mut args: impl Iterator<Item = &'a str>) -> Command {
    let mut path = None;
    let mut output = None;
    let mut pretty = true;
    while let Some(arg) = args.next() {
        match arg {
            "--compact" => pretty = false,
            "--pretty" | "--json" => pretty = true,
            "--output" | "-o" => match args.next() {
                Some(value) => output = Some(PathBuf::from(value)),
                None => return Command::Incomplete("--output needs a path".to_owned()),
            },
            other if path.is_none() && !other.starts_with('-') => {
                path = Some(PathBuf::from(other));
            }
            other => return Command::Unknown(other.to_owned()),
        }
    }
    path.map_or_else(
        || Command::Incomplete("save needs the path of a project file".to_owned()),
        |path| Command::Save {
            path,
            output,
            pretty,
        },
    )
}

/// Reads `render <file> --preset <id> --out <file> [--sequence <name>]
/// [--encoder <element>] [--range <in:out>] [--verify]`.
fn parse_render<'a>(mut args: impl Iterator<Item = &'a str>) -> Command {
    let mut path = None;
    let mut sequence = None;
    let mut preset = None;
    let mut output = None;
    let mut encoder = None;
    let mut range = None;
    let mut verify = false;
    let mut pretty = true;
    while let Some(arg) = args.next() {
        match arg {
            "--compact" => pretty = false,
            "--pretty" | "--json" => pretty = true,
            "--verify" => verify = true,
            "--sequence" => match args.next() {
                Some(value) => sequence = Some(value.to_owned()),
                None => return Command::Incomplete("--sequence needs a sequence name".to_owned()),
            },
            "--preset" => match args.next() {
                Some(value) => preset = Some(value.to_owned()),
                None => return Command::Incomplete("--preset needs a preset id".to_owned()),
            },
            "--out" | "--output" | "-o" => match args.next() {
                Some(value) => output = Some(PathBuf::from(value)),
                None => return Command::Incomplete("--out needs a path".to_owned()),
            },
            "--encoder" => match args.next() {
                Some(value) => encoder = Some(value.to_owned()),
                None => {
                    return Command::Incomplete("--encoder needs an encoder element".to_owned());
                }
            },
            "--range" => match args.next() {
                Some(value) => match render::FrameRange::parse(value) {
                    Ok(parsed) => range = Some(parsed),
                    Err(error) => return Command::Incomplete(error.message),
                },
                None => return Command::Incomplete("--range needs IN:OUT in frames".to_owned()),
            },
            other if path.is_none() && !other.starts_with('-') => {
                path = Some(PathBuf::from(other));
            }
            other => return Command::Unknown(other.to_owned()),
        }
    }
    let Some(project) = path else {
        return Command::Incomplete("render needs the path of a project file".to_owned());
    };
    let Some(preset) = preset else {
        return Command::Incomplete("render needs --preset with an export preset id".to_owned());
    };
    let Some(output) = output else {
        return Command::Incomplete("render needs --out with the file to write".to_owned());
    };
    Command::Render {
        options: Box::new(render::Options {
            project,
            sequence,
            preset,
            output,
            encoder,
            range,
            verify,
        }),
        pretty,
    }
}

/// Reads `serve [--project <file>] [--instance <name>] [--directory <dir>]`.
fn parse_serve<'a>(mut args: impl Iterator<Item = &'a str>) -> Command {
    let mut options = serve::Options::default();
    let mut pretty = true;
    while let Some(arg) = args.next() {
        match arg {
            "--compact" => pretty = false,
            "--pretty" | "--json" => pretty = true,
            "--project" => match args.next() {
                Some(value) => options.project = Some(PathBuf::from(value)),
                None => {
                    return Command::Incomplete("--project needs the path of a project".to_owned());
                }
            },
            "--instance" => match args.next() {
                Some(value) => value.clone_into(&mut options.instance),
                None => return Command::Incomplete("--instance needs a name".to_owned()),
            },
            "--directory" => match args.next() {
                Some(value) => options.directory = Some(PathBuf::from(value)),
                None => return Command::Incomplete("--directory needs a path".to_owned()),
            },
            "--plugin-dir" => match args.next() {
                Some(value) => options.plugin_dir = Some(PathBuf::from(value)),
                None => return Command::Incomplete("--plugin-dir needs a path".to_owned()),
            },
            other => return Command::Unknown(other.to_owned()),
        }
    }
    Command::Serve { options, pretty }
}

/// Reads `plugin <new|list|enable|disable|remove> [<id>] [--dir <dir>]
/// [--project <file>]`, plus the options `new` and `install` add.
fn parse_plugin<'a>(mut args: impl Iterator<Item = &'a str>) -> Command {
    let mut pretty = true;
    let mut options = plugin::Options::default();
    let Some(word) = args.next() else {
        return Command::Incomplete(plugin::NEEDS_ACTION.to_owned());
    };
    // Scaffolding takes options none of the other operations do, and none of
    // theirs, so it reads its own arguments rather than sharing this loop.
    if word == "new" {
        return parse_plugin_new(args);
    }
    let mut id = None;
    let mut dev = false;
    let mut project_local = false;
    let mut test = plugin_test::Options::default();
    while let Some(arg) = args.next() {
        match arg {
            "--compact" => pretty = false,
            "--pretty" | "--json" => pretty = true,
            "--dev" => dev = true,
            "--project-local" => project_local = true,
            "--dir" => match args.next() {
                Some(value) => options.user_dir = Some(PathBuf::from(value)),
                None => return Command::Incomplete("--dir needs a path".to_owned()),
            },
            "--project" => match args.next() {
                Some(value) => options.project = Some(PathBuf::from(value)),
                None => {
                    return Command::Incomplete("--project needs the path of a project".to_owned());
                }
            },
            "--fixture" => match args.next() {
                Some(value) => test.fixture = Some(PathBuf::from(value)),
                None => {
                    return Command::Incomplete(
                        "--fixture needs the path of a project file".to_owned(),
                    );
                }
            },
            "--args" => match args.next() {
                Some(value) => test.args = Some(value.to_owned()),
                None => {
                    return Command::Incomplete("--args needs a JSON object".to_owned());
                }
            },
            other if id.is_none() && !other.starts_with('-') => id = Some(other.to_owned()),
            other => return Command::Unknown(other.to_owned()),
        }
    }

    let needs_id = |what: &str| Command::Incomplete(format!("plugin {what} needs a plugin id"));
    let action = match (word, id) {
        ("install", Some(path)) => plugin::Action::Install {
            path: PathBuf::from(path),
            dev,
            location: project_local.then_some(InstallLocation::Project),
        },
        ("install", None) => {
            return Command::Incomplete(
                "plugin install needs the path of a built .wasm or a plugin directory".to_owned(),
            );
        }
        ("reload", Some(id)) => plugin::Action::Reload(id),
        ("reload", None) => return needs_id("reload"),
        ("test", Some(id)) => plugin::Action::Test { id, options: test },
        ("test", None) => return needs_id("test"),
        ("list", None) => plugin::Action::List,
        ("list", Some(extra)) => return Command::Unknown(extra),
        ("enable", Some(id)) => plugin::Action::Enable(id),
        ("disable", Some(id)) => plugin::Action::Disable(id),
        ("remove", Some(id)) => plugin::Action::Remove(id),
        (word @ ("enable" | "disable" | "remove"), None) => return needs_id(word),
        (other, _) => return Command::Unknown(other.to_owned()),
    };
    Command::Plugin {
        action,
        options,
        pretty,
    }
}

/// Reads `plugin new --world <world> <name> [--output <dir>] [--id <id>]
/// [--sdk-path <dir>] [--force]`.
fn parse_plugin_new<'a>(mut args: impl Iterator<Item = &'a str>) -> Command {
    let mut pretty = true;
    let mut name = None;
    let mut chosen = None;
    let mut scaffold = scaffold::Options::default();
    while let Some(arg) = args.next() {
        match arg {
            "--compact" => pretty = false,
            "--pretty" | "--json" => pretty = true,
            "--force" => scaffold.force = true,
            "--world" => match args.next() {
                Some(value) => chosen = Some(value.to_owned()),
                None => return Command::Incomplete(needs_world()),
            },
            "--output" | "-o" => match args.next() {
                Some(value) => scaffold.parent = Some(PathBuf::from(value)),
                None => return Command::Incomplete("--output needs a directory".to_owned()),
            },
            "--sdk-path" => match args.next() {
                Some(value) => scaffold.sdk_path = Some(PathBuf::from(value)),
                None => {
                    return Command::Incomplete(
                        "--sdk-path needs the path of a subordinate-sdk checkout".to_owned(),
                    );
                }
            },
            "--id" => match args.next() {
                Some(value) => scaffold.id = Some(value.to_owned()),
                None => {
                    return Command::Incomplete("--id needs a reverse-DNS plugin id".to_owned());
                }
            },
            other if name.is_none() && !other.starts_with('-') => name = Some(other.to_owned()),
            other => return Command::Unknown(other.to_owned()),
        }
    }

    let Some(name) = name else {
        return Command::Incomplete("plugin new needs a name for the plugin".to_owned());
    };
    let Some(chosen) = chosen else {
        return Command::Incomplete(needs_world());
    };
    match scaffold::parse_world(&chosen) {
        Ok(chosen) => {
            scaffold.world = chosen;
            scaffold.name = name;
            Command::Plugin {
                action: plugin::Action::New(Box::new(scaffold)),
                options: plugin::Options::default(),
                pretty,
            }
        }
        Err(error) => Command::Incomplete(error.message),
    }
}

/// What a caller is told when `plugin new` names no world.
fn needs_world() -> String {
    format!(
        "plugin new needs --world with one of {}",
        scaffold::TEMPLATED_WORLDS
            .iter()
            .map(|world| world.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// Prints `value` as JSON, one line when `pretty` is false.
fn print_json(value: &serde_json::Value, pretty: bool) -> ExitCode {
    let text = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    };
    match text {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

/// Prints what a subcommand answered, or its error as a JSON `SubError` on
/// stderr.
///
/// A failure is JSON on the same terms as a success — indented by default,
/// one line with `--compact` — so whoever called the CLI parses one shape
/// whichever way it went. A plugin failure carries the WIT item it belongs to
/// and a hint beside its stable code (`sub_plugin::errors`).
fn report(result: sub_core::SubResult<serde_json::Value>, pretty: bool) -> ExitCode {
    match result {
        Ok(value) => print_json(&value, pretty),
        Err(err) => print_error(&err, pretty),
    }
}

/// Prints one `SubError` as JSON on stderr and fails.
fn print_error(error: &sub_core::SubError, pretty: bool) -> ExitCode {
    let json = error.to_json();
    let text = if pretty {
        serde_json::to_string_pretty(&json)
    } else {
        serde_json::to_string(&json)
    };
    match text {
        Ok(text) => eprintln!("{text}"),
        // A `SubError` is always serialisable; if that ever fails, the
        // rendered error is still better than nothing.
        Err(_) => eprintln!("{error}"),
    }
    ExitCode::FAILURE
}

/// Prints a plugin subcommand's answer, failing on a report that says the
/// plugin did not pass.
///
/// `plugin test` answers a whole report whichever way the run went — a failed
/// check is data, not an error — so the JSON is printed either way and the
/// verdict it carries becomes the exit code, with its one-line summary on
/// stderr so nothing has to be parsed to see that the run failed.
fn plugin_report(result: sub_core::SubResult<serde_json::Value>, pretty: bool) -> ExitCode {
    let Ok(value) = &result else {
        return report(result, pretty);
    };
    let failed = value["ok"] == serde_json::Value::Bool(false);
    let summary = value["summary"].as_str().unwrap_or("").to_owned();
    let status = print_json(value, pretty);
    if failed {
        eprintln!("{summary}");
        return ExitCode::FAILURE;
    }
    status
}

/// Generates the Command API JSON Schema and prints it.
///
/// The document is produced by the code, never written by hand: a dispatcher
/// over a throwaway engine is asked to describe every method it serves, which
/// is the same table it dispatches through. The committed copy at
/// `docs/schema/command-api.json` is what this prints.
fn schema(pretty: bool) -> ExitCode {
    let engine = match sub_edit::Engine::spawn(sub_model::Project::new("schema")) {
        Ok(engine) => engine,
        Err(err) => return print_error(&err, pretty),
    };
    let dispatcher = sub_command::Dispatcher::new(engine.handle().clone());
    let document = sub_command::schema::document(&dispatcher);
    let status = print_json(&document, pretty);
    if let Err(err) = engine.shutdown() {
        return print_error(&err, pretty);
    }
    status
}

/// Collects the diagnostics and prints them as JSON.
fn diag(pretty: bool) -> ExitCode {
    let diagnostics = match sub_media::HardwareDiagnostics::collect() {
        Ok(diagnostics) => diagnostics,
        Err(err) => return print_error(&err, pretty),
    };
    let mut json = match diagnostics.to_json() {
        Ok(json) => json,
        Err(err) => return print_error(&err, pretty),
    };
    // The registry says which encoders are installed; the probe says which of
    // them this machine can actually start, which is what export selection
    // goes by. The panel shows both, so the CLI prints both.
    match sub_export::EncoderProbe::cached().and_then(sub_export::EncoderProbe::to_json) {
        Ok(encoders) => json["encoder_probe"] = encoders,
        Err(err) => return print_error(&err, pretty),
    }
    print_json(&json, pretty)
}

fn main() -> ExitCode {
    if let Err(err) = sub_core::logging::init("info") {
        eprintln!("{err}");
        return ExitCode::FAILURE;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Command::Version => {
            tracing::info!(
                version = env!("CARGO_PKG_VERSION"),
                "subordinate-cli starting"
            );
            println!("subordinate-cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Help => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Command::Diag { pretty } => diag(pretty),
        Command::Schema { pretty } => schema(pretty),
        Command::New {
            path,
            name,
            force,
            pretty,
        } => report(project::new(&path, name.as_deref(), force), pretty),
        Command::Open { path, pretty } => report(project::open(&path), pretty),
        Command::Save {
            path,
            output,
            pretty,
        } => report(project::save(&path, output.as_deref()), pretty),
        Command::Inspect { path, pretty } => report(project::inspect(&path), pretty),
        Command::Render { options, pretty } => report(render::run(&options), pretty),
        Command::Serve { options, pretty } => {
            // The readiness line goes out before the wait begins, so whoever
            // launched this knows the endpoint is bound without polling for a
            // socket to appear.
            let result = serve::serve(&options, |ready| {
                print_json(ready, pretty);
            });
            report(result, pretty)
        }
        Command::Plugin {
            action,
            options,
            pretty,
        } => plugin_report(plugin::run(&action, &options), pretty),
        Command::Unknown(arg) => {
            eprintln!("unknown argument: {arg}\n\n{USAGE}");
            ExitCode::FAILURE
        }
        Command::Incomplete(what) => {
            eprintln!("{what}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, InstallLocation, parse, plugin, plugin_test};
    use std::path::{Path, PathBuf};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn no_arguments_still_prints_the_version() {
        assert_eq!(parse(&args(&[])), Command::Version);
    }

    #[test]
    fn diag_defaults_to_pretty_json_and_can_be_compacted() {
        assert_eq!(parse(&args(&["diag"])), Command::Diag { pretty: true });
        assert_eq!(
            parse(&args(&["diag", "--compact"])),
            Command::Diag { pretty: false }
        );
        assert_eq!(
            parse(&args(&["diag", "--json"])),
            Command::Diag { pretty: true }
        );
    }

    #[test]
    fn schema_defaults_to_pretty_json_and_can_be_compacted() {
        assert_eq!(parse(&args(&["schema"])), Command::Schema { pretty: true });
        assert_eq!(
            parse(&args(&["schema", "--compact"])),
            Command::Schema { pretty: false }
        );
        assert_eq!(
            parse(&args(&["schema", "--verbose"])),
            Command::Unknown("--verbose".to_owned())
        );
    }

    #[test]
    fn the_project_subcommands_take_a_path() {
        assert_eq!(
            parse(&args(&["inspect", "cut.sub"])),
            Command::Inspect {
                path: "cut.sub".into(),
                pretty: true,
            }
        );
        assert_eq!(
            parse(&args(&["open", "cut.sub", "--compact"])),
            Command::Open {
                path: "cut.sub".into(),
                pretty: false,
            }
        );
        assert_eq!(
            parse(&args(&["new", "cut.sub", "--name", "Doc cut", "--force"])),
            Command::New {
                path: "cut.sub".into(),
                name: Some("Doc cut".to_owned()),
                force: true,
                pretty: true,
            }
        );
        assert_eq!(
            parse(&args(&["save", "cut.sub", "--output", "copy.sub"])),
            Command::Save {
                path: "cut.sub".into(),
                output: Some("copy.sub".into()),
                pretty: true,
            }
        );
    }

    #[test]
    fn a_subcommand_without_its_path_is_reported_rather_than_guessed_at() {
        for subcommand in ["new", "open", "save", "inspect"] {
            assert!(
                matches!(parse(&args(&[subcommand])), Command::Incomplete(_)),
                "{subcommand} accepted no path",
            );
        }
        for incomplete in [
            vec!["new", "cut.sub", "--name"],
            vec!["save", "cut.sub", "--output"],
            vec!["serve", "--project"],
            vec!["serve", "--instance"],
            vec!["serve", "--directory"],
        ] {
            assert!(
                matches!(parse(&args(&incomplete)), Command::Incomplete(_)),
                "{incomplete:?} was accepted",
            );
        }
    }

    #[test]
    fn render_takes_a_project_a_preset_an_output_and_the_overrides() {
        let Command::Render { options, pretty } = parse(&args(&[
            "render",
            "cut.sub",
            "--sequence",
            "Main cut",
            "--preset",
            "youtube-1080p",
            "--out",
            "/tmp/cut.mp4",
            "--encoder",
            "x264enc",
            "--range",
            "24:48",
            "--verify",
            "--compact",
        ])) else {
            panic!("render did not parse");
        };
        assert_eq!(
            *options,
            super::render::Options {
                project: PathBuf::from("cut.sub"),
                sequence: Some("Main cut".to_owned()),
                preset: "youtube-1080p".to_owned(),
                output: PathBuf::from("/tmp/cut.mp4"),
                encoder: Some("x264enc".to_owned()),
                range: Some(super::render::FrameRange {
                    start: 24,
                    end: Some(48),
                }),
                verify: true,
            }
        );
        assert!(!pretty);
    }

    #[test]
    fn render_reports_what_it_is_missing_rather_than_guessing() {
        for incomplete in [
            vec!["render"],
            vec!["render", "cut.sub"],
            vec!["render", "cut.sub", "--preset", "youtube-1080p"],
            vec!["render", "cut.sub", "--out", "/tmp/cut.mp4"],
            vec!["render", "cut.sub", "--preset"],
            vec!["render", "cut.sub", "--out"],
            vec!["render", "cut.sub", "--sequence"],
            vec!["render", "cut.sub", "--encoder"],
            vec!["render", "cut.sub", "--range"],
            vec![
                "render",
                "cut.sub",
                "--preset",
                "youtube-1080p",
                "--out",
                "/tmp/cut.mp4",
                "--range",
                "nonsense",
            ],
        ] {
            assert!(
                matches!(parse(&args(&incomplete)), Command::Incomplete(_)),
                "{incomplete:?} was accepted",
            );
        }
        assert_eq!(
            parse(&args(&[
                "render",
                "cut.sub",
                "--preset",
                "youtube-1080p",
                "--out",
                "/tmp/cut.mp4",
                "--fast",
            ])),
            Command::Unknown("--fast".to_owned())
        );
    }

    #[test]
    fn serve_defaults_to_the_per_user_endpoint_and_no_project() {
        assert_eq!(
            parse(&args(&["serve"])),
            Command::Serve {
                options: super::serve::Options::default(),
                pretty: true,
            }
        );
        let Command::Serve { options, pretty } = parse(&args(&[
            "serve",
            "--project",
            "cut.sub",
            "--instance",
            "ci",
            "--directory",
            "/tmp/run",
            "--compact",
        ])) else {
            panic!("serve did not parse");
        };
        assert_eq!(options.project.as_deref(), Some(Path::new("cut.sub")));
        assert_eq!(options.instance, "ci");
        assert_eq!(options.directory.as_deref(), Some(Path::new("/tmp/run")));
        assert!(!pretty);
    }

    #[test]
    fn plugin_parses_its_four_operations() {
        let Command::Plugin {
            action,
            options,
            pretty,
        } = parse(&args(&[
            "plugin",
            "list",
            "--dir",
            "/data/plugins",
            "--compact",
        ]))
        else {
            panic!("plugin list did not parse");
        };
        assert_eq!(action, plugin::Action::List);
        assert_eq!(
            options.user_dir.as_deref(),
            Some(Path::new("/data/plugins"))
        );
        assert!(options.project.is_none());
        assert!(!pretty);

        for (word, build) in [
            ("enable", plugin::Action::Enable as fn(String) -> _),
            ("disable", plugin::Action::Disable),
            ("remove", plugin::Action::Remove),
        ] {
            let Command::Plugin { action, .. } = parse(&args(&[
                "plugin",
                word,
                "com.example.one",
                "--project",
                "cut.sub",
            ])) else {
                panic!("plugin {word} did not parse");
            };
            assert_eq!(action, build("com.example.one".to_owned()));
        }

        let Command::Plugin { options, .. } =
            parse(&args(&["plugin", "list", "--project", "cut.sub"]))
        else {
            panic!("plugin list did not parse");
        };
        assert_eq!(options.project.as_deref(), Some(Path::new("cut.sub")));
    }

    #[test]
    fn plugin_install_takes_a_path_and_the_dev_flag() {
        let Command::Plugin { action, .. } = parse(&args(&[
            "plugin",
            "install",
            "./target/wasm32-wasip2/release/one.wasm",
            "--dev",
        ])) else {
            panic!("plugin install did not parse");
        };
        assert_eq!(
            action,
            plugin::Action::Install {
                path: PathBuf::from("./target/wasm32-wasip2/release/one.wasm"),
                dev: true,
                location: None,
            }
        );

        let Command::Plugin { action, .. } =
            parse(&args(&["plugin", "install", "./plugin", "--project-local"]))
        else {
            panic!("plugin install did not parse");
        };
        assert_eq!(
            action,
            plugin::Action::Install {
                path: PathBuf::from("./plugin"),
                dev: false,
                location: Some(InstallLocation::Project),
            }
        );

        assert!(matches!(
            parse(&args(&["plugin", "install"])),
            Command::Incomplete(_)
        ));
    }

    #[test]
    fn plugin_new_takes_a_world_a_name_and_where_to_write_it() {
        let Command::Plugin { action, .. } = parse(&args(&[
            "plugin",
            "new",
            "--world",
            "effect",
            "tint",
            "--output",
            "/tmp/plugins",
            "--id",
            "com.example.tint",
            "--sdk-path",
            "/checkout/sdk/subordinate-sdk",
            "--force",
        ])) else {
            panic!("plugin new did not parse");
        };
        assert_eq!(
            action,
            plugin::Action::New(Box::new(super::scaffold::Options {
                world: sub_plugin::manifest::World::Effect,
                name: "tint".to_owned(),
                parent: Some(PathBuf::from("/tmp/plugins")),
                id: Some("com.example.tint".to_owned()),
                sdk_path: Some(PathBuf::from("/checkout/sdk/subordinate-sdk")),
                force: true,
            })),
        );
    }

    #[test]
    fn plugin_new_reports_a_missing_world_a_missing_name_and_an_untemplated_world() {
        for incomplete in [
            vec!["plugin", "new"],
            vec!["plugin", "new", "demo"],
            vec!["plugin", "new", "--world"],
            vec!["plugin", "new", "--world", "panel", "demo"],
            vec!["plugin", "new", "--world", "command", "demo", "--output"],
            vec!["plugin", "new", "--world", "command", "demo", "--id"],
            vec!["plugin", "new", "--world", "command", "demo", "--sdk-path"],
        ] {
            assert!(
                matches!(parse(&args(&incomplete)), Command::Incomplete(_)),
                "{incomplete:?} was accepted",
            );
        }
    }

    #[test]
    fn plugin_reload_takes_an_id() {
        let Command::Plugin { action, .. } = parse(&args(&["plugin", "reload", "com.example.one"]))
        else {
            panic!("plugin reload did not parse");
        };
        assert_eq!(action, plugin::Action::Reload("com.example.one".to_owned()));
        assert!(matches!(
            parse(&args(&["plugin", "reload"])),
            Command::Incomplete(_)
        ));
    }

    #[test]
    fn plugin_reports_a_missing_subcommand_or_id() {
        assert_eq!(
            parse(&args(&["plugin"])),
            Command::Incomplete(plugin::NEEDS_ACTION.to_owned())
        );
        assert!(matches!(
            parse(&args(&["plugin", "enable"])),
            Command::Incomplete(_)
        ));
        assert_eq!(
            parse(&args(&["plugin", "publish"])),
            Command::Unknown("publish".to_owned())
        );
        assert_eq!(
            parse(&args(&["plugin", "list", "com.example.one"])),
            Command::Unknown("com.example.one".to_owned())
        );
    }

    #[test]
    fn plugin_test_takes_an_id_a_fixture_and_arguments() {
        let Command::Plugin {
            action, options, ..
        } = parse(&args(&[
            "plugin",
            "test",
            "com.example.one",
            "--fixture",
            "/edits/demo.sub",
            "--args",
            "{\"threshold\":-40}",
            "--dir",
            "/data/plugins",
        ]))
        else {
            panic!("plugin test did not parse");
        };
        assert_eq!(
            action,
            plugin::Action::Test {
                id: "com.example.one".to_owned(),
                options: plugin_test::Options {
                    fixture: Some(PathBuf::from("/edits/demo.sub")),
                    args: Some("{\"threshold\":-40}".to_owned()),
                },
            }
        );
        assert_eq!(
            options.user_dir.as_deref(),
            Some(Path::new("/data/plugins"))
        );

        assert!(matches!(
            parse(&args(&["plugin", "test"])),
            Command::Incomplete(_)
        ));
        assert!(matches!(
            parse(&args(&["plugin", "test", "com.example.one", "--fixture"])),
            Command::Incomplete(_)
        ));
    }

    #[test]
    fn serve_takes_a_plugin_directory() {
        let Command::Serve { options, .. } =
            parse(&args(&["serve", "--plugin-dir", "/data/plugins"]))
        else {
            panic!("serve did not parse");
        };
        assert_eq!(
            options.plugin_dir.as_deref(),
            Some(Path::new("/data/plugins"))
        );
    }

    #[test]
    fn unknown_arguments_are_reported_rather_than_ignored() {
        assert_eq!(
            parse(&args(&["transcode"])),
            Command::Unknown("transcode".to_owned())
        );
        assert_eq!(
            parse(&args(&["diag", "--verbose"])),
            Command::Unknown("--verbose".to_owned())
        );
        assert_eq!(
            parse(&args(&["inspect", "cut.sub", "second.sub"])),
            Command::Unknown("second.sub".to_owned())
        );
    }

    #[test]
    fn help_is_offered_under_every_spelling() {
        for spelling in ["--help", "-h", "help"] {
            assert_eq!(parse(&args(&[spelling])), Command::Help);
        }
        for subcommand in [
            "diag", "schema", "new", "open", "save", "inspect", "render", "serve", "plugin",
        ] {
            assert!(
                super::USAGE.contains(subcommand),
                "{subcommand} is undocumented",
            );
        }
    }
}
