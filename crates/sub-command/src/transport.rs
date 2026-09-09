//! The local socket transport: JSON-RPC over a Unix socket or a named pipe.
//!
//! [`Server`] binds the [`Endpoint`], publishes its lock file and serves every
//! client that connects, each on its own thread. The framing is
//! newline-delimited JSON: one message per line, one response line per request,
//! nothing at all for a notification. That is the simplest framing an agent, a
//! shell script or a non-Rust client can speak, and it needs no length prefix.
//!
//! [`Client`] is the other half: it finds a running server through the lock
//! file and speaks the same framing. `subordinate-cli` and `subordinate-mcp`
//! use it, and so do the tests.
//!
//! Nothing here interprets a message: bytes go to [`Dispatcher::handle_text`]
//! and its answer goes back out, so the socket surface is exactly the
//! in-process surface (decision-7).
//!
//! ```
//! use std::sync::Arc;
//!
//! use sub_command::endpoint::Endpoint;
//! use sub_command::transport::{Client, Server};
//! use sub_command::Dispatcher;
//! use sub_edit::Engine;
//! use sub_model::Project;
//!
//! let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
//! let directory = std::env::temp_dir().join(format!("sub-doc-{}", std::process::id()));
//! let endpoint = Endpoint::in_directory(directory, "doctest").unwrap();
//! let server = Server::bind(endpoint, Arc::new(Dispatcher::new(engine.handle().clone()))).unwrap();
//!
//! let mut client = Client::connect(server.endpoint()).unwrap();
//! let revision = client.invoke("project.revision", None).unwrap();
//! assert_eq!(revision["revision"], 0);
//!
//! drop(client);
//! server.shutdown().unwrap();
//! engine.shutdown().unwrap();
//! ```

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use interprocess::TryClone as _;
use interprocess::local_socket::traits::{Listener as _, Stream as _};
use interprocess::local_socket::{
    GenericFilePath, ListenerNonblockingMode, ListenerOptions, Name, Stream, ToFsName,
};
use serde_json::Value;
use sub_core::{SubError, SubResult, codes as core_codes};
use tracing::{debug, warn};

use crate::codes;
use crate::endpoint::{Address, Endpoint, LockFile};
use crate::rpc::{Notification, Request, RequestId, Response};
use crate::{Dispatcher, RpcError};

/// The longest single message the server will read, in bytes.
///
/// A project of any plausible size serialises well under this; a client that
/// sends more has lost its framing, and reading on would only grow a buffer
/// until the process died.
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// How long the accept loop waits between polls when no client is knocking.
///
/// Accept is non-blocking so that [`Server::shutdown`] is prompt; this is the
/// price paid for it, and it is well under human perception.
const ACCEPT_POLL: Duration = Duration::from_millis(5);

/// How long [`Client::connect_to`] keeps trying an address that is there but
/// momentarily busy.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether a failed connection means "come back in a moment" rather than
/// "nobody is there".
///
/// On Windows a named pipe whose instances are all in use answers
/// `ERROR_PIPE_BUSY` while the server is between accepts, which is the ordinary
/// state of a busy server rather than an error.
#[cfg(windows)]
fn is_busy(error: &std::io::Error) -> bool {
    const ERROR_PIPE_BUSY: i32 = 231;
    error.raw_os_error() == Some(ERROR_PIPE_BUSY) || error.kind() == ErrorKind::WouldBlock
}

/// Whether a failed connection means "come back in a moment".
#[cfg(not(windows))]
fn is_busy(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::WouldBlock
}

impl Address {
    /// The address as the local socket implementation names it.
    ///
    /// On Windows a `\\.\pipe\…` path maps to that named pipe; on Unix a path
    /// maps to that socket file.
    ///
    /// # Errors
    ///
    /// Returns `command.endpoint_unavailable` when the platform cannot express
    /// this address.
    pub(crate) fn to_name(&self) -> SubResult<Name<'static>> {
        std::path::PathBuf::from(self.to_wire())
            .to_fs_name::<GenericFilePath>()
            .map_err(|error| {
                SubError::new(
                    codes::ENDPOINT_UNAVAILABLE,
                    "the address is not usable on this platform",
                )
                .with_detail("address", self.to_wire())
                .with_cause(&error)
            })
    }
}

/// A running Command API server.
///
/// Binding publishes the lock file; dropping the server (or calling
/// [`Server::shutdown`]) takes the address and the lock file away again, so a
/// clean exit leaves nothing behind for the next start to clean up.
#[derive(Debug)]
pub struct Server {
    endpoint: Endpoint,
    stop: Arc<AtomicBool>,
    accepting: Option<JoinHandle<()>>,
    connections: Arc<AtomicUsize>,
}

impl Server {
    /// Binds `endpoint` and starts serving `dispatcher` on it.
    ///
    /// Any socket and lock file left by a crashed instance are removed first;
    /// see [`clear_stale`].
    ///
    /// # Errors
    ///
    /// Returns `command.address_in_use` when another live server already holds
    /// the endpoint, `command.endpoint_unavailable` when the directory or the
    /// address cannot be used, and `command.transport_io` when the listener
    /// cannot be created.
    pub fn bind(endpoint: Endpoint, dispatcher: Arc<Dispatcher>) -> SubResult<Self> {
        endpoint.create_directory()?;
        clear_stale(&endpoint)?;

        let listener = ListenerOptions::new()
            .name(endpoint.address().to_name()?)
            .nonblocking(ListenerNonblockingMode::Accept)
            .create_sync()
            .map_err(|error| {
                let code = if error.kind() == ErrorKind::AddrInUse {
                    codes::ADDRESS_IN_USE
                } else {
                    codes::TRANSPORT_IO
                };
                SubError::new(code, "the endpoint could not be bound")
                    .with_detail("address", endpoint.address().to_wire())
                    .with_cause(&error)
            })?;
        restrict_socket(endpoint.address())?;
        LockFile::for_endpoint(&endpoint).write(&endpoint.lock_path())?;

        let stop = Arc::new(AtomicBool::new(false));
        let connections = Arc::new(AtomicUsize::new(0));
        let accepting = {
            let stop = Arc::clone(&stop);
            let connections = Arc::clone(&connections);
            let address = endpoint.address().to_wire();
            thread::Builder::new()
                .name("sub-command-accept".to_owned())
                .spawn(move || {
                    debug!(address = %address, "the Command API is listening");
                    accept_loop(&listener, &dispatcher, &stop, &connections);
                    debug!(address = %address, "the Command API has stopped listening");
                })
                .map_err(|error| {
                    SubError::new(
                        codes::TRANSPORT_IO,
                        "the accept thread could not be started",
                    )
                    .with_cause(&error)
                })?
        };

        Ok(Self {
            endpoint,
            stop,
            accepting: Some(accepting),
            connections,
        })
    }

    /// The endpoint being served.
    #[must_use]
    pub const fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// How many client connections are open right now.
    #[must_use]
    pub fn connection_count(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }

    /// Stops accepting, releases the address and removes the lock file.
    ///
    /// Connections already open end when their client disconnects; they are not
    /// waited for, because a client that never speaks again must not be able to
    /// hold the editor's exit open.
    ///
    /// # Errors
    ///
    /// Returns `core.io` when the lock file or the socket cannot be removed.
    pub fn shutdown(mut self) -> SubResult<()> {
        self.stop_serving()
    }

    /// The body of both [`Server::shutdown`] and [`Drop`].
    fn stop_serving(&mut self) -> SubResult<()> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(accepting) = self.accepting.take()
            && accepting.join().is_err()
        {
            warn!("the accept thread panicked");
        }
        // The listener unlinks its own socket file as it drops; the lock file
        // is ours to remove.
        remove_if_present(&self.endpoint.lock_path())?;
        if let Some(path) = self.endpoint.address().socket_path() {
            remove_if_present(path)?;
        }
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Err(error) = self.stop_serving() {
            warn!(%error, "the endpoint could not be cleaned up");
        }
    }
}

/// Accepts clients until `stop` is set, giving each one its own thread.
fn accept_loop(
    listener: &interprocess::local_socket::Listener,
    dispatcher: &Arc<Dispatcher>,
    stop: &Arc<AtomicBool>,
    connections: &Arc<AtomicUsize>,
) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok(stream) => {
                // On the BSDs an accepted socket inherits the listener's
                // non-blocking flag, so a stream taken from an
                // accept-non-blocking listener would answer `WouldBlock`
                // instead of waiting for its client's next message. Asking for
                // blocking explicitly costs nothing where it is already so.
                if let Err(error) = stream.set_nonblocking(false) {
                    warn!(%error, "a client stream could not be made blocking");
                }
                let dispatcher = Arc::clone(dispatcher);
                let connections = Arc::clone(connections);
                connections.fetch_add(1, Ordering::SeqCst);
                if let Err(error) = thread::Builder::new()
                    .name("sub-command-client".to_owned())
                    .spawn(move || {
                        serve(&stream, &dispatcher);
                        connections.fetch_sub(1, Ordering::SeqCst);
                    })
                {
                    warn!(%error, "a client thread could not be started");
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => thread::sleep(ACCEPT_POLL),
            Err(error) => {
                warn!(%error, "a client could not be accepted");
                thread::sleep(ACCEPT_POLL);
            }
        }
    }
}

/// Serves one connection until its client goes away.
fn serve(stream: &Stream, dispatcher: &Dispatcher) {
    let mut reader = BufReader::new(stream);
    let mut writer = stream;
    let mut line = Vec::new();
    loop {
        line.clear();
        match read_message(&mut reader, &mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) => {
                if error.kind() != ErrorKind::UnexpectedEof {
                    warn!(%error, "a client connection failed while reading");
                }
                let _ = respond(&mut writer, &oversized_response(&error));
                return;
            }
        }
        let text = trim_end(&line);
        if text.is_empty() {
            continue;
        }
        let answer = match std::str::from_utf8(text) {
            Ok(text) => dispatcher.handle_text(text),
            Err(error) => Some(
                serde_json::to_string(&Response::error(
                    None,
                    crate::rpc::error_codes::parse_error(error.to_string()),
                ))
                .unwrap_or_else(|_| String::from(r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"parse error"},"id":null}"#)),
            ),
        };
        if let Some(answer) = answer
            && let Err(error) = respond(&mut writer, &answer)
        {
            debug!(%error, "a client connection failed while writing");
            return;
        }
    }
}

/// Writes one message and its newline.
fn respond(writer: &mut &Stream, message: &str) -> std::io::Result<()> {
    writer.write_all(message.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// The response sent to a client that broke the framing before the connection
/// is dropped.
fn oversized_response(error: &std::io::Error) -> String {
    let failure = SubError::new(codes::TRANSPORT_IO, "the message could not be read")
        .with_detail("reason", error.to_string())
        .with_detail("limit_bytes", MAX_MESSAGE_BYTES);
    serde_json::to_string(&Response::error(None, RpcError::from_sub_error(&failure)))
        .unwrap_or_default()
}

/// Reads one newline-terminated message into `buffer`, refusing to grow past
/// [`MAX_MESSAGE_BYTES`].
///
/// Returns the number of bytes read, which is zero only at a clean end of
/// stream.
fn read_message(reader: &mut impl BufRead, buffer: &mut Vec<u8>) -> std::io::Result<usize> {
    loop {
        let (complete, used) = {
            let available = match reader.fill_buf() {
                Ok(available) => available,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if available.is_empty() {
                return Ok(buffer.len());
            }
            if let Some(at) = available.iter().position(|byte| *byte == b'\n') {
                buffer.extend_from_slice(&available[..=at]);
                (true, at + 1)
            } else {
                buffer.extend_from_slice(available);
                (false, available.len())
            }
        };
        reader.consume(used);
        if complete {
            return Ok(buffer.len());
        }
        if buffer.len() > MAX_MESSAGE_BYTES {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                format!("a message longer than {MAX_MESSAGE_BYTES} bytes was refused"),
            ));
        }
    }
}

/// Drops the line ending a message arrived with.
fn trim_end(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &line[..end]
}

/// Removes a socket and lock file left behind by an instance that died.
///
/// The lock file names the address of the last server. If something still
/// answers there, this endpoint belongs to a live process and binding must
/// fail; if nothing answers, the socket file is a corpse and is unlinked so the
/// new server can take the address.
///
/// # Errors
///
/// Returns `command.address_in_use` when a live server holds the endpoint, and
/// `core.io` when a stale file cannot be removed.
pub fn clear_stale(endpoint: &Endpoint) -> SubResult<()> {
    let lock_path = endpoint.lock_path();
    let recorded = match LockFile::read(&lock_path) {
        Ok(lock) => lock,
        Err(error) => {
            warn!(%error, "ignoring an unreadable lock file");
            None
        }
    };
    let mut addresses = vec![endpoint.address().clone()];
    let recorded_address = recorded.as_ref().and_then(|lock| lock.address().ok());
    if let Some(address) = recorded_address
        && !addresses.contains(&address)
    {
        addresses.push(address);
    }

    for address in &addresses {
        if answers(address) {
            return Err(SubError::new(
                codes::ADDRESS_IN_USE,
                "another instance is already listening",
            )
            .with_detail("address", address.to_wire())
            .with_detail("pid", recorded.as_ref().map(|lock| lock.pid)));
        }
        if let Some(path) = address.socket_path()
            && path.exists()
        {
            warn!(path = %path.display(), "removing a socket left by a dead instance");
            remove_if_present(path)?;
        }
    }
    if recorded.is_some() {
        warn!(path = %lock_path.display(), "removing a lock file left by a dead instance");
    }
    remove_if_present(&lock_path)
}

/// Whether anything is listening at `address` right now.
///
/// A refused connection or a missing socket file means nobody is; any other
/// failure is treated as somebody being there, because deleting a file we do
/// not understand is worse than refusing to start.
fn answers(address: &Address) -> bool {
    let Ok(name) = address.to_name() else {
        return false;
    };
    // A path that exists but is not a socket cannot have a server behind it.
    // Linux refuses the connection, but the BSDs answer `ENOTSOCK`, which has
    // no stable `ErrorKind`, so the file type settles it before connecting.
    if !is_socket_or_absent(address) {
        return false;
    }
    match Stream::connect(name) {
        Ok(_) => true,
        Err(error) => !matches!(
            error.kind(),
            ErrorKind::NotFound | ErrorKind::ConnectionRefused
        ),
    }
}

/// Whether `address`'s socket path is a socket, or is not a path at all.
///
/// A Windows named pipe has no path to inspect, and a path that is simply
/// missing is settled by the connection attempt itself.
#[cfg(unix)]
fn is_socket_or_absent(address: &Address) -> bool {
    use std::os::unix::fs::FileTypeExt as _;

    let Some(path) = address.socket_path() else {
        return true;
    };
    match std::fs::metadata(path) {
        Ok(metadata) => metadata.file_type().is_socket(),
        Err(_) => true,
    }
}

/// Windows has no socket file to inspect.
#[cfg(not(unix))]
const fn is_socket_or_absent(_address: &Address) -> bool {
    true
}

/// Removes a file, treating "it was not there" as success.
fn remove_if_present(path: &Path) -> SubResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(
            SubError::new(core_codes::IO, "a stale file could not be removed")
                .with_detail("path", path.display().to_string())
                .with_cause(&error),
        ),
    }
}

/// Restricts a freshly bound socket to its owner.
#[cfg(unix)]
fn restrict_socket(address: &Address) -> SubResult<()> {
    use std::os::unix::fs::PermissionsExt;

    use sub_core::ResultExt;

    let Some(path) = address.socket_path() else {
        return Ok(());
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).sub_context(
        codes::ENDPOINT_UNAVAILABLE,
        "the socket could not be made private",
    )
}

/// A named pipe inherits the default DACL, which already excludes other users.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn restrict_socket(_address: &Address) -> SubResult<()> {
    Ok(())
}

/// A connection to a running Command API server.
///
/// The client is deliberately blocking and single-threaded: one message out,
/// one message back. That is what a CLI invocation and the MCP bridge's request
/// loop want, and it keeps the framing obvious.
#[derive(Debug)]
pub struct Client {
    reader: BufReader<Stream>,
    writer: Stream,
    next_id: i64,
}

impl Client {
    /// Connects to the server that published `endpoint`'s lock file.
    ///
    /// # Errors
    ///
    /// Returns `command.not_running` when no lock file is there,
    /// `command.lock_file_invalid` when it cannot be understood, and
    /// `command.transport_io` when the connection fails.
    pub fn connect(endpoint: &Endpoint) -> SubResult<Self> {
        let lock = endpoint.read_lock()?.ok_or_else(|| {
            SubError::new(codes::NOT_RUNNING, "no server has published this endpoint")
                .with_detail("lock_file", endpoint.lock_path().display().to_string())
        })?;
        Self::connect_to(&lock.address()?)
    }

    /// Connects to an address directly, without consulting a lock file.
    ///
    /// A server that is between accepts has no free pipe instance on Windows,
    /// so a connection that is merely *busy* is retried for
    /// [`CONNECT_TIMEOUT`] rather than reported as a failure.
    ///
    /// # Errors
    ///
    /// Returns `command.not_running` when nothing is listening, and
    /// `command.transport_io` for any other failure.
    pub fn connect_to(address: &Address) -> SubResult<Self> {
        let name = address.to_name()?;
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let stream = loop {
            match Stream::connect(name.clone()) {
                Ok(stream) => break stream,
                Err(error) if is_busy(&error) && Instant::now() < deadline => {
                    thread::sleep(ACCEPT_POLL);
                }
                Err(error) => {
                    let code = if matches!(
                        error.kind(),
                        ErrorKind::NotFound | ErrorKind::ConnectionRefused
                    ) {
                        codes::NOT_RUNNING
                    } else {
                        codes::TRANSPORT_IO
                    };
                    return Err(SubError::new(code, "the Command API could not be reached")
                        .with_detail("address", address.to_wire())
                        .with_cause(&error));
                }
            }
        };
        let writer = stream.try_clone().map_err(|error| {
            SubError::new(
                codes::TRANSPORT_IO,
                "the connection could not be duplicated",
            )
            .with_cause(&error)
        })?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
            next_id: 1,
        })
    }

    /// Sends one message, adding the newline the framing needs.
    ///
    /// # Errors
    ///
    /// Returns `command.transport_io` when the message cannot be written, and
    /// `command.invalid_params` when it contains a newline of its own.
    pub fn send(&mut self, message: &str) -> SubResult<()> {
        if message.contains('\n') {
            return Err(SubError::new(
                codes::INVALID_PARAMS,
                "a message may not contain a newline",
            ));
        }
        self.writer
            .write_all(message.as_bytes())
            .and_then(|()| self.writer.write_all(b"\n"))
            .and_then(|()| self.writer.flush())
            .map_err(|error| {
                SubError::new(codes::TRANSPORT_IO, "the message could not be sent")
                    .with_cause(&error)
            })
    }

    /// Reads one message, or `None` when the server closed the connection.
    ///
    /// # Errors
    ///
    /// Returns `command.transport_io` when the connection fails.
    pub fn receive(&mut self) -> SubResult<Option<String>> {
        let mut line = Vec::new();
        let read = read_message(&mut self.reader, &mut line).map_err(|error| {
            SubError::new(codes::TRANSPORT_IO, "the reply could not be read").with_cause(&error)
        })?;
        if read == 0 {
            return Ok(None);
        }
        let text = String::from_utf8(trim_end(&line).to_vec()).map_err(|error| {
            SubError::new(codes::TRANSPORT_IO, "the reply was not valid UTF-8").with_cause(&error)
        })?;
        Ok(Some(text))
    }

    /// Sends a notification, which is never answered.
    ///
    /// # Errors
    ///
    /// Returns `command.transport_io` when the message cannot be sent.
    pub fn notify(&mut self, notification: &Notification) -> SubResult<()> {
        let text = serde_json::to_string(notification).map_err(|error| {
            SubError::new(
                core_codes::INTERNAL,
                "the notification could not be encoded",
            )
            .with_cause(&error)
        })?;
        self.send(&text)
    }

    /// Sends one request and reads its response.
    ///
    /// # Errors
    ///
    /// Returns `command.transport_io` when the connection fails or the server
    /// answers a different request, and whatever the request itself produced.
    pub fn call(&mut self, request: &Request) -> SubResult<Response> {
        let text = serde_json::to_string(request).map_err(|error| {
            SubError::new(core_codes::INTERNAL, "the request could not be encoded")
                .with_cause(&error)
        })?;
        self.send(&text)?;
        let reply = self.receive()?.ok_or_else(|| {
            SubError::new(
                codes::TRANSPORT_IO,
                "the server closed the connection without answering",
            )
        })?;
        let response: Response = serde_json::from_str(&reply).map_err(|error| {
            SubError::new(codes::TRANSPORT_IO, "the reply was not a JSON-RPC response")
                .with_detail("reply", reply.clone())
                .with_cause(&error)
        })?;
        if response.id.as_ref() != Some(&request.id) && !response.is_error() {
            return Err(
                SubError::new(codes::TRANSPORT_IO, "the reply answered another request")
                    .with_detail("expected", request.id.to_string()),
            );
        }
        Ok(response)
    }

    /// Calls a method and returns its result, turning a JSON-RPC error back
    /// into the [`SubError`] the server raised.
    ///
    /// # Errors
    ///
    /// Returns the server's error, or `command.transport_io` when the
    /// connection fails.
    pub fn invoke(&mut self, method: &str, params: Option<Value>) -> SubResult<Value> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let response = self.call(&Request::new(RequestId::Number(id), method, params))?;
        match response.value() {
            Some(value) => Ok(value.clone()),
            None => Err(response.error_ref().map_or_else(
                || {
                    SubError::new(
                        codes::TRANSPORT_IO,
                        "the reply had neither result nor error",
                    )
                },
                |error| {
                    error.sub_error().unwrap_or_else(|| {
                        SubError::new(codes::TRANSPORT_IO, error.message.clone())
                            .with_detail("rpc_code", error.code)
                    })
                },
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::BufReader;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use serde_json::json;
    use sub_edit::Engine;
    use sub_model::Project;

    use super::{Client, LockFile, MAX_MESSAGE_BYTES, Server, clear_stale, read_message, trim_end};
    use crate::codes;
    use crate::endpoint::Endpoint;
    use crate::rpc::{Notification, Request, RequestId};
    use crate::{Dispatcher, Response};

    /// A directory nothing else in this test binary uses.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sub-transport-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An engine, a dispatcher and a server bound to a private endpoint.
    fn serving(name: &str) -> (Engine, Server) {
        let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
        let endpoint = Endpoint::in_directory(temp_dir(name), "test").unwrap();
        let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
        let server = Server::bind(endpoint, dispatcher).unwrap();
        (engine, server)
    }

    #[test]
    fn a_client_runs_a_command_and_the_engine_sees_it() {
        let (engine, server) = serving("one-client");
        let mut client = Client::connect(server.endpoint()).unwrap();

        let applied = client
            .invoke("bin.create", Some(json!({ "name": "Footage" })))
            .unwrap();
        assert_eq!(applied["revision"], 1);
        assert_eq!(
            engine.handle().snapshot().root_bin.children[0].name,
            "Footage",
        );

        let revision = client.invoke("project.revision", None).unwrap();
        assert_eq!(revision["revision"], 1);

        drop(client);
        server.shutdown().unwrap();
        engine.shutdown().unwrap();
    }

    #[test]
    fn the_lock_file_is_how_a_client_finds_the_server() {
        let (engine, server) = serving("discovery");
        let lock = server.endpoint().read_lock().unwrap().unwrap();
        assert_eq!(lock.pid, std::process::id());
        assert_eq!(lock.address, server.endpoint().address().to_wire());
        assert_eq!(lock.transport, server.endpoint().address().transport());

        // A client that knows only the directory finds the address in the file.
        let same = Endpoint::in_directory(server.endpoint().directory(), "test").unwrap();
        let mut client = Client::connect(&same).unwrap();
        assert_eq!(
            client.invoke("project.revision", None).unwrap()["revision"],
            0,
        );

        drop(client);
        server.shutdown().unwrap();
        engine.shutdown().unwrap();
    }

    #[test]
    fn several_clients_are_served_at_once() {
        let (engine, server) = serving("concurrent");
        let clients = 6;
        let mut connections: Vec<Client> = (0..clients)
            .map(|_| Client::connect(server.endpoint()).unwrap())
            .collect();

        // Interleave the clients so none is finished before the next starts.
        for (index, client) in connections.iter_mut().enumerate() {
            let name = format!("Bin {index}");
            let applied = client
                .invoke("bin.create", Some(json!({ "name": name })))
                .unwrap();
            assert_eq!(applied["revision"], u64::try_from(index).unwrap() + 1);
        }
        for client in &mut connections {
            assert_eq!(
                client.invoke("project.revision", None).unwrap()["revision"],
                u64::try_from(clients).unwrap(),
            );
        }

        let deadline = Instant::now() + Duration::from_secs(5);
        while server.connection_count() < clients && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(server.connection_count(), clients);

        assert_eq!(engine.handle().snapshot().root_bin.children.len(), clients);
        drop(connections);
        server.shutdown().unwrap();
        engine.shutdown().unwrap();
    }

    #[test]
    fn framing_is_one_message_a_line() {
        let (engine, server) = serving("framing");
        let mut client = Client::connect(server.endpoint()).unwrap();

        // A notification is answered with nothing at all, so the next answer
        // belongs to the request that follows it.
        client
            .notify(&Notification::new(
                "bin.create",
                Some(json!({ "name": "Quiet" })),
            ))
            .unwrap();
        let response = client
            .call(&Request::new(
                RequestId::Text("abc".to_owned()),
                "project.revision",
                None,
            ))
            .unwrap();
        assert_eq!(response.id, Some(RequestId::Text("abc".to_owned())));
        assert_eq!(response.value().unwrap()["revision"], 1);

        // A blank line is skipped rather than answered.
        client.send("").unwrap();
        assert_eq!(
            client.invoke("project.revision", None).unwrap()["revision"],
            1,
        );

        // Malformed JSON becomes a parse error, and the connection survives it.
        client.send("{ not json").unwrap();
        let reply: Response = serde_json::from_str(&client.receive().unwrap().unwrap()).unwrap();
        assert_eq!(reply.error_ref().unwrap().code, -32700);
        assert_eq!(reply.id, None);
        assert_eq!(
            client.invoke("project.revision", None).unwrap()["revision"],
            1,
        );

        // A batch is answered with a batch, still on one line.
        client
            .send(concat!(
                r#"[{"jsonrpc":"2.0","method":"project.revision","id":1},"#,
                r#"{"jsonrpc":"2.0","method":"project.revision","id":2}]"#,
            ))
            .unwrap();
        let batch: Vec<Response> =
            serde_json::from_str(&client.receive().unwrap().unwrap()).unwrap();
        assert_eq!(batch.len(), 2);

        drop(client);
        server.shutdown().unwrap();
        engine.shutdown().unwrap();
    }

    #[test]
    fn an_engine_error_comes_back_as_the_sub_error_it_was() {
        let (engine, server) = serving("errors");
        let mut client = Client::connect(server.endpoint()).unwrap();
        let error = client.invoke("nope.method", None).unwrap_err();
        assert_eq!(error.code, codes::UNKNOWN_METHOD);
        drop(client);
        server.shutdown().unwrap();
        engine.shutdown().unwrap();
    }

    #[test]
    fn a_message_may_not_carry_a_newline() {
        let (engine, server) = serving("newline");
        let mut client = Client::connect(server.endpoint()).unwrap();
        assert_eq!(
            client.send("{}\n{}").unwrap_err().code,
            codes::INVALID_PARAMS,
        );
        drop(client);
        server.shutdown().unwrap();
        engine.shutdown().unwrap();
    }

    #[test]
    fn a_stale_socket_and_lock_file_are_cleaned_up_on_start() {
        let directory = temp_dir("stale");
        let endpoint = Endpoint::in_directory(&directory, "test").unwrap();
        endpoint.create_directory().unwrap();

        // What a crashed instance leaves behind: a lock file naming an address
        // nothing is listening on and, on Unix, the socket file itself.
        LockFile::for_endpoint(&endpoint)
            .write(&endpoint.lock_path())
            .unwrap();
        if let Some(path) = endpoint.address().socket_path() {
            std::fs::write(path, b"").unwrap();
            assert!(path.exists());
        }

        clear_stale(&endpoint).unwrap();
        assert!(!endpoint.lock_path().exists());
        assert!(
            endpoint
                .address()
                .socket_path()
                .is_none_or(|path| !path.exists())
        );

        // And a server binds over the wreckage without complaint.
        let engine = Engine::spawn(Project::new("Doc cut")).unwrap();
        let dispatcher = Arc::new(Dispatcher::new(engine.handle().clone()));
        let endpoint = Endpoint::in_directory(&directory, "test").unwrap();
        let server = Server::bind(endpoint, dispatcher).unwrap();
        let mut client = Client::connect(server.endpoint()).unwrap();
        assert_eq!(
            client.invoke("project.revision", None).unwrap()["revision"],
            0,
        );
        drop(client);
        server.shutdown().unwrap();
        engine.shutdown().unwrap();
    }

    #[test]
    fn an_unreadable_lock_file_is_treated_as_wreckage() {
        let directory = temp_dir("junk-lock");
        let endpoint = Endpoint::in_directory(&directory, "test").unwrap();
        endpoint.create_directory().unwrap();
        std::fs::write(endpoint.lock_path(), "{ not json").unwrap();
        clear_stale(&endpoint).unwrap();
        assert!(!endpoint.lock_path().exists());
    }

    #[test]
    fn a_live_server_is_not_evicted() {
        let (engine, server) = serving("in-use");
        let other = Engine::spawn(Project::new("Doc cut")).unwrap();
        let same = Endpoint::in_directory(server.endpoint().directory(), "test").unwrap();
        let error =
            Server::bind(same, Arc::new(Dispatcher::new(other.handle().clone()))).unwrap_err();
        assert_eq!(error.code, codes::ADDRESS_IN_USE);

        // The live server is untouched.
        let mut client = Client::connect(server.endpoint()).unwrap();
        assert_eq!(
            client.invoke("project.revision", None).unwrap()["revision"],
            0,
        );

        drop(client);
        other.shutdown().unwrap();
        server.shutdown().unwrap();
        engine.shutdown().unwrap();
    }

    #[test]
    fn shutdown_takes_the_address_and_the_lock_file_away() {
        let (engine, server) = serving("shutdown");
        let endpoint = server.endpoint().clone();
        server.shutdown().unwrap();
        assert!(!endpoint.lock_path().exists());
        assert!(
            endpoint
                .address()
                .socket_path()
                .is_none_or(|path| !path.exists())
        );
        assert_eq!(
            Client::connect(&endpoint).unwrap_err().code,
            codes::NOT_RUNNING,
        );
        engine.shutdown().unwrap();
    }

    #[test]
    fn connecting_where_nothing_runs_says_so() {
        let endpoint = Endpoint::in_directory(temp_dir("absent"), "test").unwrap();
        assert_eq!(
            Client::connect(&endpoint).unwrap_err().code,
            codes::NOT_RUNNING,
        );
        assert_eq!(
            Client::connect_to(endpoint.address()).unwrap_err().code,
            codes::NOT_RUNNING,
        );
    }

    #[test]
    fn messages_are_read_one_line_at_a_time_and_bounded() {
        let mut reader = BufReader::new(&b"one\r\ntwo\nthree"[..]);
        let mut line = Vec::new();
        for expected in ["one", "two", "three"] {
            line.clear();
            assert!(read_message(&mut reader, &mut line).unwrap() > 0);
            assert_eq!(trim_end(&line), expected.as_bytes());
        }
        line.clear();
        assert_eq!(read_message(&mut reader, &mut line).unwrap(), 0);

        let flood = vec![b'x'; MAX_MESSAGE_BYTES + 1];
        let mut reader = BufReader::new(&flood[..]);
        line.clear();
        let error = read_message(&mut reader, &mut line).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn an_over_long_message_is_refused_without_killing_the_server() {
        let (engine, server) = serving("oversized");
        let mut greedy = Client::connect(server.endpoint()).unwrap();
        let flood = "x".repeat(MAX_MESSAGE_BYTES + 1);
        // The server gives up on this connection, so the send or the read fails.
        let _ = greedy.send(&flood);
        let _ = greedy.receive();
        drop(greedy);

        let mut client = Client::connect(server.endpoint()).unwrap();
        assert_eq!(
            client.invoke("project.revision", None).unwrap()["revision"],
            0,
        );
        drop(client);
        server.shutdown().unwrap();
        engine.shutdown().unwrap();
    }

    #[test]
    fn a_closed_connection_releases_its_thread() {
        let (engine, server) = serving("release");
        {
            let mut client = Client::connect(server.endpoint()).unwrap();
            assert_eq!(
                client.invoke("project.revision", None).unwrap()["revision"],
                0,
            );
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while server.connections.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(server.connection_count(), 0);
        server.shutdown().unwrap();
        engine.shutdown().unwrap();
    }
}
