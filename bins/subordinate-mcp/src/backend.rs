//! The other end of the bridge: the running editor's Command API.
//!
//! The bridge never owns project state. It finds the editor the way any other
//! client does — through the lock file the endpoint publishes — and speaks
//! JSON-RPC to it over the local socket or named pipe
//! (`sub_command::transport`). When no editor is listening it starts
//! `subordinate-cli serve`, which is the same engine and the same dispatcher
//! with nothing drawn, waits for its readiness line and connects to the address
//! that line announces (docs/PLAN.md §7).
//!
//! A server this bridge launched belongs to it: closing the child's standard
//! input is how `serve` is asked to stop, so the headless engine goes away with
//! the agent's session rather than outliving it.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use serde_json::Value;
use sub_command::endpoint::{Address, DEFAULT_INSTANCE, Endpoint, Transport};
use sub_command::transport::Client;
use sub_core::{SubError, SubResult};
use tracing::{debug, info, warn};

use crate::codes;

/// The environment variable naming the instance to connect to.
pub const INSTANCE_ENV: &str = "SUBORDINATE_INSTANCE";
/// The environment variable overriding where the endpoint lives.
pub const DIRECTORY_ENV: &str = "SUBORDINATE_ENDPOINT_DIR";
/// The environment variable naming the headless CLI to launch.
pub const CLI_ENV: &str = "SUBORDINATE_CLI";
/// The environment variable that forbids launching one (`1`, `true`, `yes`).
pub const NO_LAUNCH_ENV: &str = "SUBORDINATE_MCP_NO_LAUNCH";

/// The name of the headless CLI, without an executable suffix.
pub const CLI_STEM: &str = "subordinate-cli";

/// The file name of the headless CLI, with this platform's executable suffix.
#[must_use]
pub fn cli_name() -> String {
    format!("{CLI_STEM}{}", std::env::consts::EXE_SUFFIX)
}

/// Where the bridge looks for the editor, and what it may do when it is absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The instance name the endpoint is derived from.
    pub instance: String,
    /// Where the socket and lock file live, overriding the per-user default.
    pub directory: Option<PathBuf>,
    /// The headless CLI to launch, when it is not the one beside this binary.
    pub cli: Option<PathBuf>,
    /// Whether a headless server may be started when none is running.
    pub launch: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            instance: DEFAULT_INSTANCE.to_owned(),
            directory: None,
            cli: None,
            launch: true,
        }
    }
}

impl Options {
    /// The options this process was started with.
    ///
    /// An MCP server is launched by its client from a `.mcp.json` entry, which
    /// carries environment but no arguments worth parsing, so every knob is an
    /// environment variable.
    #[must_use]
    pub fn from_env() -> Self {
        let mut options = Self::default();
        if let Ok(instance) = std::env::var(INSTANCE_ENV)
            && !instance.is_empty()
        {
            options.instance = instance;
        }
        options.directory = std::env::var_os(DIRECTORY_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        options.cli = std::env::var_os(CLI_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        options.launch = !std::env::var(NO_LAUNCH_ENV).is_ok_and(|value| is_true(&value));
        options
    }

    /// The endpoint these options name.
    ///
    /// # Errors
    ///
    /// Returns `command.invalid_instance` for an unusable instance name and
    /// `command.endpoint_unavailable` when this machine offers no per-user
    /// address.
    pub fn endpoint(&self) -> SubResult<Endpoint> {
        match &self.directory {
            Some(directory) => Endpoint::in_directory(directory.clone(), &self.instance),
            None => Endpoint::for_instance(&self.instance),
        }
    }
}

/// Whether an environment variable spells "yes".
fn is_true(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// A connection to the Command API, and the server it started, if it started
/// one.
#[derive(Debug)]
pub struct Backend {
    /// The connection. One at a time: a JSON-RPC call is a round trip on it.
    client: Mutex<Client>,
    /// The address that was connected to, for diagnostics.
    address: Address,
    /// The headless server this bridge launched, which it also stops.
    server: Option<Headless>,
}

impl Backend {
    /// Connects to the editor, launching a headless one if that is allowed.
    ///
    /// # Errors
    ///
    /// Returns `command.not_running` when nothing is listening and launching
    /// is switched off, `mcp.cli_not_found` when the headless CLI cannot be
    /// located, `mcp.launch_failed` when it will not start or does not
    /// announce itself, and the `command.*` codes for a connection that fails.
    pub fn connect(options: &Options) -> SubResult<Self> {
        let endpoint = options.endpoint()?;
        match Client::connect(&endpoint) {
            Ok(client) => {
                let address = endpoint.address().clone();
                info!(address = %address.to_wire(), "connected to a running editor");
                Ok(Self {
                    client: Mutex::new(client),
                    address,
                    server: None,
                })
            }
            Err(error) if error.code == sub_command::codes::NOT_RUNNING && options.launch => {
                debug!("no editor is running; starting one headlessly");
                Self::launch(options)
            }
            Err(error) => Err(error),
        }
    }

    /// Starts `subordinate-cli serve` and connects to what it announces.
    fn launch(options: &Options) -> SubResult<Self> {
        let executable = cli_path(options.cli.as_deref())?;
        let mut command = Command::new(&executable);
        command.arg("serve").arg("--compact");
        command.arg("--instance").arg(&options.instance);
        if let Some(directory) = &options.directory {
            command.arg("--directory").arg(directory);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| {
                SubError::new(codes::LAUNCH_FAILED, "the headless server would not start")
                    .with_detail("executable", executable.display().to_string())
                    .with_cause(&error)
            })?;

        let mut stdout = None;
        let announced = readiness(&mut child, &mut stdout)
            .and_then(|line| announced_address(&line))
            .and_then(|address| {
                info!(address = %address.to_wire(), "started a headless server");
                Client::connect_to(&address).map(|client| (address, client))
            });
        let (address, client) = match announced {
            Ok(pair) => pair,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.with_detail("executable", executable.display().to_string()));
            }
        };
        Ok(Self {
            client: Mutex::new(client),
            address,
            server: Some(Headless { child, stdout }),
        })
    }

    /// The address the bridge is talking to.
    #[must_use]
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// Whether this bridge started the server it is talking to.
    #[must_use]
    pub fn launched_server(&self) -> bool {
        self.server.is_some()
    }

    /// Calls a Command API method and returns its result.
    ///
    /// # Errors
    ///
    /// Returns whatever the method itself returned, or `command.transport_io`
    /// when the connection fails.
    pub fn invoke(&self, method: &str, params: Option<Value>) -> SubResult<Value> {
        let mut client = self
            .client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        client.invoke(method, params)
    }
}

/// A headless server this bridge started and is responsible for stopping.
#[derive(Debug)]
struct Headless {
    /// The `subordinate-cli serve` process.
    child: Child,
    /// Its standard output, held open past the readiness line: `serve` prints
    /// a closing report there, and a pipe closed early would end it with a
    /// broken pipe instead of letting it shut down.
    stdout: Option<BufReader<ChildStdout>>,
}

impl Drop for Headless {
    fn drop(&mut self) {
        // `serve` stops when its standard input reaches end of file; that is a
        // clean shutdown, where killing it would leave the lock file behind.
        drop(self.child.stdin.take());
        if let Some(mut stdout) = self.stdout.take() {
            let mut report = String::new();
            match stdout.read_to_string(&mut report) {
                Ok(_) => debug!(report = report.trim(), "the headless server signed off"),
                Err(error) => warn!(%error, "the headless server's report was lost"),
            }
        }
        match self.child.wait() {
            Ok(status) => debug!(%status, "the headless server stopped"),
            Err(error) => warn!(%error, "the headless server could not be reaped"),
        }
    }
}

/// Reads the one-line readiness announcement `serve` prints when it is bound.
///
/// The reader is handed back through `stdout` so the pipe stays open for the
/// rest of the server's life.
fn readiness(child: &mut Child, stdout: &mut Option<BufReader<ChildStdout>>) -> SubResult<Value> {
    let pipe = child.stdout.take().ok_or_else(|| {
        SubError::new(
            codes::LAUNCH_FAILED,
            "the headless server has no standard output",
        )
    })?;
    let reader = stdout.insert(BufReader::new(pipe));
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|error| {
        SubError::new(
            codes::LAUNCH_FAILED,
            "the headless server did not announce itself",
        )
        .with_cause(&error)
    })?;
    serde_json::from_str(&line).map_err(|error| {
        SubError::new(
            codes::LAUNCH_FAILED,
            "the headless server's readiness line was not JSON",
        )
        .with_detail("line", line.clone())
        .with_cause(&error)
    })
}

/// The address a readiness line announces.
fn announced_address(announcement: &Value) -> SubResult<Address> {
    let incomplete = |what: &str| {
        SubError::new(
            codes::LAUNCH_FAILED,
            format!("the readiness line has no {what}"),
        )
        .with_detail("line", announcement.clone())
    };
    let transport = announcement
        .get("transport")
        .cloned()
        .ok_or_else(|| incomplete("transport"))?;
    let transport: Transport =
        serde_json::from_value(transport).map_err(|_| incomplete("known transport"))?;
    let wire = announcement
        .get("address")
        .and_then(Value::as_str)
        .ok_or_else(|| incomplete("address"))?;
    Address::from_wire(transport, wire)
}

/// Where the headless CLI is.
///
/// A packaged install puts it beside this binary; a cargo build puts it one
/// directory up from the test or example that is running. `SUBORDINATE_CLI`
/// overrides both, which is how a client points the bridge at a build of its
/// own.
fn cli_path(configured: Option<&Path>) -> SubResult<PathBuf> {
    if let Some(path) = configured {
        return Ok(path.to_path_buf());
    }
    let executable = std::env::current_exe().map_err(|error| {
        SubError::new(
            codes::CLI_NOT_FOUND,
            "this executable's own path is not known",
        )
        .with_cause(&error)
    })?;
    let directory = executable.parent().ok_or_else(|| {
        SubError::new(codes::CLI_NOT_FOUND, "this executable has no directory")
            .with_detail("executable", executable.display().to_string())
    })?;
    let name = cli_name();
    let beside = directory.join(&name);
    if beside.is_file() {
        return Ok(beside);
    }
    if let Some(above) = directory.parent() {
        let candidate = above.join(&name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(
        SubError::new(codes::CLI_NOT_FOUND, "the headless CLI could not be found")
            .with_detail("searched", directory.display().to_string())
            .with_detail("name", name)
            .with_detail("variable", CLI_ENV),
    )
}

#[cfg(test)]
mod tests {
    use super::{Options, announced_address, cli_path, is_true};
    use serde_json::json;
    use std::path::Path;
    use sub_command::endpoint::{DEFAULT_INSTANCE, Transport};

    #[test]
    fn the_default_is_the_user_instance_and_launching_is_allowed() {
        let options = Options::default();
        assert_eq!(options.instance, DEFAULT_INSTANCE);
        assert!(options.launch);
        assert!(options.directory.is_none());
        assert_eq!(
            options.endpoint().expect("an endpoint").instance(),
            DEFAULT_INSTANCE,
        );
    }

    #[test]
    fn a_directory_sends_the_endpoint_somewhere_of_its_own() {
        let options = Options {
            directory: Some("/tmp/sub-mcp-options".into()),
            instance: "test".to_owned(),
            ..Options::default()
        };
        let endpoint = options.endpoint().expect("an endpoint in a directory");
        assert_eq!(endpoint.directory(), Path::new("/tmp/sub-mcp-options"));
        assert_eq!(endpoint.instance(), "test");
    }

    #[test]
    fn the_readiness_line_says_where_to_connect() {
        let address = announced_address(&json!({
            "transport": "unix_socket",
            "address": "/tmp/sub-mcp/default.sock",
        }))
        .expect("an address");
        assert_eq!(address.transport(), Transport::UnixSocket);
        assert_eq!(address.to_wire(), "/tmp/sub-mcp/default.sock");
    }

    #[test]
    fn a_readiness_line_that_says_nothing_useful_is_a_launch_failure() {
        for line in [
            json!({}),
            json!({ "transport": "carrier_pigeon", "address": "/tmp/x" }),
            json!({ "transport": "unix_socket" }),
        ] {
            let error = announced_address(&line).expect_err("not an address");
            assert_eq!(error.code.as_str(), "mcp.launch_failed");
        }
    }

    #[test]
    fn a_configured_cli_path_is_taken_as_it_is() {
        let path = cli_path(Some(Path::new("/opt/subordinate/subordinate-cli"))).expect("the path");
        assert_eq!(path, Path::new("/opt/subordinate/subordinate-cli"));
    }

    #[test]
    fn only_the_usual_spellings_of_yes_switch_launching_off() {
        assert!(is_true("1"));
        assert!(is_true("TRUE"));
        assert!(is_true(" yes "));
        assert!(!is_true("0"));
        assert!(!is_true(""));
    }
}
