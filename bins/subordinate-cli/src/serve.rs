//! The `serve` subcommand: the Command API without a GUI.
//!
//! `subordinate-mcp` and any other client find a running editor through the
//! endpoint's lock file (docs/PLAN.md §7). When no GUI is running, this is what
//! they launch: the same engine, the same [`Dispatcher`] and the same socket or
//! named pipe, with nothing drawn.
//!
//! The readiness line comes first — one JSON object on stdout naming the
//! transport, the address and the lock file — so a supervising process knows
//! the endpoint is bound before it connects, rather than polling for a socket
//! to appear. The server then runs until stdin reaches end of file, which is
//! how a parent process asks it to stop; a terminal user ends it with Ctrl-D or
//! Ctrl-C.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use sub_command::Dispatcher;
use sub_command::endpoint::{DEFAULT_INSTANCE, Endpoint};
use sub_command::transport::Server;
use sub_core::{ResultExt, SubResult, codes};
use sub_edit::Engine;
use sub_model::{Project, json as project_json};
use sub_plugin::registry::{self, PluginDirs, PluginRegistry};

/// How `serve` was asked to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The project to load, or a new empty one when absent.
    pub project: Option<PathBuf>,
    /// The instance name the endpoint is derived from.
    pub instance: String,
    /// Where the socket and lock file live, overriding the per-user default.
    /// Tests set it; a user has no reason to.
    pub directory: Option<PathBuf>,
    /// The per-user plugin directory, overriding the platform default.
    pub plugin_dir: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            project: None,
            instance: DEFAULT_INSTANCE.to_owned(),
            directory: None,
            plugin_dir: None,
        }
    }
}

/// The project name a `serve` with no `--project` starts from.
const UNTITLED: &str = "Untitled";

/// Serves the Command API until stdin reaches end of file.
///
/// `ready` is handed the readiness JSON once the endpoint is bound and before
/// the wait begins; it is the caller's business to print it.
///
/// # Errors
///
/// Returns `core.io` when a named project cannot be read, whatever the model
/// returns for a project this build cannot load, `command.address_in_use` when
/// another live instance already holds the endpoint, and the other
/// `command.*` codes when the endpoint cannot be bound.
pub fn serve(options: &Options, ready: impl FnOnce(&Value)) -> SubResult<Value> {
    let project = load(options.project.as_deref())?;
    let name = project.name.clone();

    let engine = Engine::spawn(project)?;
    let mut dispatcher = Dispatcher::new(engine.handle().clone());
    install_plugin_methods(&mut dispatcher, options)?;
    let dispatcher = Arc::new(dispatcher);
    let served = run(options, &dispatcher, &name, ready);
    let stopped = engine.shutdown();
    let report = served?;
    stopped?;
    Ok(report)
}

/// Puts `plugin.list`, `plugin.enable`, `plugin.disable` and `plugin.remove` on
/// the dispatcher, so an agent manages plugins through the MCP bridge exactly
/// as a user does through the CLI (docs/PLAN.md §6.4).
///
/// A machine with no per-user data directory is served without them rather
/// than not served at all: nothing else the Command API does depends on the
/// plugin directories.
fn install_plugin_methods(dispatcher: &mut Dispatcher, options: &Options) -> SubResult<()> {
    let user = match options.plugin_dir.clone() {
        Some(dir) => dir,
        None => match registry::default_user_dir() {
            Ok(dir) => dir,
            Err(error) => {
                tracing::warn!(
                    code = error.code.as_str(),
                    "plugin management is not served: {}",
                    error.message,
                );
                return Ok(());
            }
        },
    };
    let dirs = PluginDirs::new(user);
    let dirs = match options.project.as_deref() {
        Some(project) => dirs.with_project_file(project),
        None => dirs,
    };
    registry::register_methods(dispatcher, Arc::new(PluginRegistry::new(dirs)))
}

/// Binds the endpoint, announces it, waits for stdin to close and shuts down.
fn run(
    options: &Options,
    dispatcher: &Arc<Dispatcher>,
    name: &str,
    ready: impl FnOnce(&Value),
) -> SubResult<Value> {
    let endpoint = match &options.directory {
        Some(directory) => Endpoint::in_directory(directory.clone(), &options.instance)?,
        None => Endpoint::for_instance(&options.instance)?,
    };
    let announcement = announcement(&endpoint, name, options.project.as_deref());
    let server = Server::bind(endpoint, Arc::clone(dispatcher))?;
    ready(&announcement);

    let mut sink = Vec::new();
    std::io::stdin()
        .read_to_end(&mut sink)
        .sub_context(codes::IO, "the standard input of the server failed")?;

    let connections = server.connection_count();
    server.shutdown()?;
    let mut report = announcement;
    report["stopped"] = Value::Bool(true);
    report["open_connections_at_exit"] = json!(connections);
    Ok(report)
}

/// The readiness line: everything a client needs to reach this server.
fn announcement(endpoint: &Endpoint, name: &str, path: Option<&Path>) -> Value {
    json!({
        "instance": endpoint.instance(),
        "transport": endpoint.address().transport().as_str(),
        "address": endpoint.address().to_wire(),
        "lock_file": endpoint.lock_path().display().to_string(),
        "pid": std::process::id(),
        "project": {
            "name": name,
            "path": path.map(|path| path.display().to_string()),
        },
    })
}

/// The project the server starts with: the one named, or an empty one.
fn load(path: Option<&Path>) -> SubResult<Project> {
    let Some(path) = path else {
        return Ok(Project::new(UNTITLED));
    };
    let text = std::fs::read_to_string(path)
        .sub_context(codes::IO, "the project file could not be read")
        .map_err(|error| error.with_detail("path", path.display().to_string()))?;
    project_json::from_json(&text)
        .map_err(|error| error.with_detail("path", path.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::{Options, UNTITLED, announcement, load};
    use std::path::Path;
    use sub_command::endpoint::{DEFAULT_INSTANCE, Endpoint};

    #[test]
    fn serving_without_a_project_starts_an_empty_one() {
        let project = load(None).expect("an empty project needs no file");
        assert_eq!(project.name, UNTITLED);
        assert!(project.sequences.is_empty());
        assert_eq!(Options::default().instance, DEFAULT_INSTANCE);
    }

    #[test]
    fn a_missing_project_file_is_reported_with_its_path() {
        let error = load(Some(Path::new("/nowhere/absent.sub"))).expect_err("no such file");
        assert_eq!(error.code.as_str(), "core.io");
        assert!(error.to_json().to_string().contains("absent.sub"));
    }

    #[test]
    fn the_readiness_line_names_the_address_and_the_lock_file() {
        let endpoint = Endpoint::in_directory("/tmp/subordinate-cli-serve-test", "cli-test")
            .expect("an endpoint in a directory");
        let line = announcement(&endpoint, "Doc cut", Some(Path::new("doc.sub")));
        assert_eq!(line["instance"], "cli-test");
        assert_eq!(line["address"], endpoint.address().to_wire());
        assert!(
            line["lock_file"]
                .as_str()
                .expect("a lock file path")
                .ends_with("cli-test.lock.json")
        );
        assert_eq!(line["project"]["name"], "Doc cut");
        assert_eq!(line["project"]["path"], "doc.sub");
    }
}
