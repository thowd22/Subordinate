//! Where the Command API listens, and how a client finds it.
//!
//! One running editor serves one *endpoint*: a Unix-domain socket on Linux and
//! macOS, a named pipe on Windows (decision-7). The address is derived from the
//! current user and an instance name, so two users on one machine never share a
//! channel and two instances of the editor never fight over one address:
//!
//! | Platform | Directory | Address |
//! | --- | --- | --- |
//! | Linux | `$XDG_RUNTIME_DIR/subordinate` | `<directory>/<instance>.sock` |
//! | macOS | `$TMPDIR/subordinate` (per-user) | `<directory>/<instance>.sock` |
//! | Windows | `%LOCALAPPDATA%\Subordinate\run` | `\\.\pipe\subordinate.<user>.<instance>` |
//!
//! Every endpoint also has a [`LockFile`]: a small JSON file in the same
//! directory naming the transport, the address and the process id. That file is
//! how `subordinate-mcp`, `subordinate-cli` and anything else discovers a
//! running editor without guessing at platform rules, and it is what lets a new
//! server tell a live predecessor from a socket left behind by a crash.
//!
//! ```
//! use sub_command::endpoint::{Endpoint, Transport};
//!
//! let endpoint = Endpoint::in_directory("/tmp/subordinate-demo", "default").unwrap();
//! assert_eq!(endpoint.instance(), "default");
//! assert!(endpoint.lock_path().ends_with("default.lock.json"));
//! if cfg!(windows) {
//!     assert_eq!(endpoint.address().transport(), Transport::WindowsNamedPipe);
//! } else {
//!     assert_eq!(endpoint.address().transport(), Transport::UnixSocket);
//! }
//! ```

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sub_core::{ResultExt, SubError, SubResult, codes as core_codes};

use crate::codes;

/// The instance every process uses unless it is told otherwise.
pub const DEFAULT_INSTANCE: &str = "default";

/// The longest instance name accepted.
pub const MAX_INSTANCE_LEN: usize = 64;

/// The longest Unix socket path accepted.
///
/// `sockaddr_un.sun_path` holds 108 bytes on Linux and 104 on macOS, including
/// the terminating NUL. Refusing anything longer here turns a confusing
/// `bind` failure into a clear error naming the path.
pub const MAX_UNIX_SOCKET_PATH: usize = 100;

/// The IPC primitive an [`Address`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// A Unix-domain stream socket, used on Linux and macOS.
    UnixSocket,
    /// A Windows named pipe.
    WindowsNamedPipe,
}

impl Transport {
    /// The transport this build listens on.
    #[must_use]
    pub const fn native() -> Self {
        if cfg!(windows) {
            Self::WindowsNamedPipe
        } else {
            Self::UnixSocket
        }
    }

    /// The wire name used in a [`LockFile`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnixSocket => "unix_socket",
            Self::WindowsNamedPipe => "windows_named_pipe",
        }
    }
}

impl fmt::Display for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A concrete address a client can connect to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Address {
    /// A Unix-domain socket at this filesystem path.
    UnixSocket(PathBuf),
    /// A Windows named pipe, held without its `\\.\pipe\` prefix.
    WindowsNamedPipe(String),
}

/// The prefix every Windows named pipe path carries.
const PIPE_PREFIX: &str = r"\\.\pipe\";

impl Address {
    /// Which primitive this address names.
    #[must_use]
    pub const fn transport(&self) -> Transport {
        match self {
            Self::UnixSocket(_) => Transport::UnixSocket,
            Self::WindowsNamedPipe(_) => Transport::WindowsNamedPipe,
        }
    }

    /// The address as a client outside Rust would spell it: the socket path, or
    /// the full `\\.\pipe\…` path.
    ///
    /// This is the string a [`LockFile`] carries.
    #[must_use]
    pub fn to_wire(&self) -> String {
        match self {
            Self::UnixSocket(path) => path.display().to_string(),
            Self::WindowsNamedPipe(name) => format!("{PIPE_PREFIX}{name}"),
        }
    }

    /// Rebuilds an address from the transport and string a lock file recorded.
    ///
    /// # Errors
    ///
    /// Returns `command.lock_file_invalid` when the string is empty or, for a
    /// named pipe, is not under `\\.\pipe\`.
    pub fn from_wire(transport: Transport, wire: &str) -> SubResult<Self> {
        if wire.is_empty() {
            return Err(SubError::new(
                codes::LOCK_FILE_INVALID,
                "the recorded address is empty",
            ));
        }
        match transport {
            Transport::UnixSocket => Ok(Self::UnixSocket(PathBuf::from(wire))),
            Transport::WindowsNamedPipe => wire.strip_prefix(PIPE_PREFIX).map_or_else(
                || {
                    Err(
                        SubError::new(codes::LOCK_FILE_INVALID, "the pipe path is not a pipe path")
                            .with_detail("address", wire),
                    )
                },
                |name| Ok(Self::WindowsNamedPipe(name.to_owned())),
            ),
        }
    }

    /// The socket file this address occupies, if it has one.
    ///
    /// A named pipe has no filesystem entry, so nothing has to be unlinked when
    /// a Windows server dies.
    #[must_use]
    pub fn socket_path(&self) -> Option<&Path> {
        match self {
            Self::UnixSocket(path) => Some(path),
            Self::WindowsNamedPipe(_) => None,
        }
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_wire())
    }
}

/// One server's address plus the lock file that advertises it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    instance: String,
    directory: PathBuf,
    address: Address,
}

impl Endpoint {
    /// The endpoint of the [`DEFAULT_INSTANCE`] for the current user.
    ///
    /// # Errors
    ///
    /// Returns `command.endpoint_unavailable` when the platform offers no
    /// per-user runtime directory.
    pub fn user_default() -> SubResult<Self> {
        Self::for_instance(DEFAULT_INSTANCE)
    }

    /// The endpoint of a named instance for the current user.
    ///
    /// A second editor process picks a different instance name rather than
    /// evicting the first.
    ///
    /// # Errors
    ///
    /// Returns `command.invalid_instance` for a name that is empty, too long or
    /// not made of ASCII letters, digits, `.`, `-` and `_`, and
    /// `command.endpoint_unavailable` when the platform offers no per-user
    /// runtime directory.
    pub fn for_instance(instance: &str) -> SubResult<Self> {
        let instance = check_instance(instance)?;
        let directory = runtime_directory()?;
        let address = native_address(&directory, &instance, None)?;
        Ok(Self {
            instance,
            directory,
            address,
        })
    }

    /// An endpoint whose lock file — and, on Unix, whose socket — lives under
    /// `directory` instead of the per-user runtime directory.
    ///
    /// Tests and sandboxed instances use this. On Windows the pipe namespace is
    /// flat, so the directory is folded into the pipe name to keep two
    /// directories with the same instance name apart.
    ///
    /// # Errors
    ///
    /// Returns `command.invalid_instance` for a malformed instance name, and
    /// `command.endpoint_unavailable` when the resulting socket path would be
    /// longer than [`MAX_UNIX_SOCKET_PATH`].
    pub fn in_directory(directory: impl Into<PathBuf>, instance: &str) -> SubResult<Self> {
        let instance = check_instance(instance)?;
        let directory = directory.into();
        let address = native_address(&directory, &instance, Some(fold(&directory)))?;
        Ok(Self {
            instance,
            directory,
            address,
        })
    }

    /// The instance name.
    #[must_use]
    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// The directory holding the lock file.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The address clients connect to.
    #[must_use]
    pub const fn address(&self) -> &Address {
        &self.address
    }

    /// The lock file that advertises this endpoint.
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.directory.join(format!("{}.lock.json", self.instance))
    }

    /// Creates the directory, private to this user, if it is not there yet.
    ///
    /// # Errors
    ///
    /// Returns `command.endpoint_unavailable` when the directory cannot be
    /// created or cannot be made private.
    pub fn create_directory(&self) -> SubResult<()> {
        fs::create_dir_all(&self.directory).sub_context(
            codes::ENDPOINT_UNAVAILABLE,
            "the runtime directory could not be created",
        )?;
        private_directory(&self.directory)
    }

    /// The lock file this endpoint advertises, or `None` when no server has
    /// written one.
    ///
    /// # Errors
    ///
    /// Returns `command.lock_file_invalid` when the file is there but is not a
    /// lock file this build understands.
    pub fn read_lock(&self) -> SubResult<Option<LockFile>> {
        LockFile::read(&self.lock_path())
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.address)
    }
}

/// What a running server publishes so clients can find it.
///
/// The file is JSON so that a shell script, an agent or a non-Rust client can
/// read it. Unknown fields are rejected: a lock file from a future build is a
/// clear error rather than a half-understood address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockFile {
    /// The instance the server serves.
    pub instance: String,
    /// The IPC primitive in use.
    pub transport: Transport,
    /// The address, as [`Address::to_wire`] spells it.
    pub address: String,
    /// The process id of the server, for diagnostics.
    pub pid: u32,
    /// The version of the crate that wrote the file.
    pub version: String,
}

impl LockFile {
    /// The lock file a server at `endpoint` publishes.
    #[must_use]
    pub fn for_endpoint(endpoint: &Endpoint) -> Self {
        Self {
            instance: endpoint.instance().to_owned(),
            transport: endpoint.address().transport(),
            address: endpoint.address().to_wire(),
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// The address recorded in the file.
    ///
    /// # Errors
    ///
    /// Returns `command.lock_file_invalid` when the recorded address does not
    /// match its transport.
    pub fn address(&self) -> SubResult<Address> {
        Address::from_wire(self.transport, &self.address)
    }

    /// Reads a lock file, or `None` when there is none.
    ///
    /// # Errors
    ///
    /// Returns `command.lock_file_invalid` when the file exists but does not
    /// parse, and `core.io` when it cannot be read.
    pub fn read(path: &Path) -> SubResult<Option<Self>> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(
                    SubError::new(core_codes::IO, "the lock file could not be read")
                        .with_detail("path", path.display().to_string())
                        .with_cause(&error),
                );
            }
        };
        let lock: Self = serde_json::from_str(&text).map_err(|error| {
            SubError::new(
                codes::LOCK_FILE_INVALID,
                "the lock file could not be parsed",
            )
            .with_detail("path", path.display().to_string())
            .with_cause(&error)
        })?;
        Ok(Some(lock))
    }

    /// Writes the file so a reader never sees half of it: a temporary file in
    /// the same directory, renamed into place.
    ///
    /// # Errors
    ///
    /// Returns `core.io` when the file cannot be written or renamed.
    pub fn write(&self, path: &Path) -> SubResult<()> {
        let text = serde_json::to_string_pretty(self).map_err(|error| {
            SubError::new(core_codes::INTERNAL, "the lock file could not be encoded")
                .with_cause(&error)
        })? + "\n";
        let temporary = path.with_extension(format!("tmp{}", std::process::id()));
        fs::write(&temporary, text)
            .and_then(|()| fs::rename(&temporary, path))
            .map_err(|error| {
                let _ = fs::remove_file(&temporary);
                SubError::new(core_codes::IO, "the lock file could not be written")
                    .with_detail("path", path.display().to_string())
                    .with_cause(&error)
            })
    }
}

/// Validates an instance name, which becomes part of a path or a pipe name.
fn check_instance(instance: &str) -> SubResult<String> {
    let ok = !instance.is_empty()
        && instance.len() <= MAX_INSTANCE_LEN
        && instance != "."
        && instance != ".."
        && instance
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    if ok {
        Ok(instance.to_owned())
    } else {
        Err(SubError::new(
            codes::INVALID_INSTANCE,
            "an instance name must be 1 to 64 ASCII letters, digits, '.', '-' or '_'",
        )
        .with_detail("instance", instance))
    }
}

/// The address for this platform.
///
/// `discriminator` distinguishes endpoints that share an instance name but not
/// a directory; it matters only where the namespace is flat.
fn native_address(
    directory: &Path,
    instance: &str,
    discriminator: Option<String>,
) -> SubResult<Address> {
    if cfg!(windows) {
        let mut name = format!("subordinate.{}.{instance}", current_user());
        if let Some(discriminator) = discriminator {
            name.push('.');
            name.push_str(&discriminator);
        }
        Ok(Address::WindowsNamedPipe(name))
    } else {
        let path = directory.join(format!("{instance}.sock"));
        let length = path.as_os_str().len();
        if length > MAX_UNIX_SOCKET_PATH {
            return Err(SubError::new(
                codes::ENDPOINT_UNAVAILABLE,
                "the socket path is too long for a Unix-domain socket",
            )
            .with_detail("path", path.display().to_string())
            .with_detail("length", length)
            .with_detail("limit", MAX_UNIX_SOCKET_PATH));
        }
        Ok(Address::UnixSocket(path))
    }
}

/// A short, stable digest of a path: FNV-1a, so every process agrees on it
/// regardless of the standard library's hasher of the day.
fn fold(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The current user's login name, reduced to characters that are safe in a
/// path or a pipe name.
fn current_user() -> String {
    let raw = ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .unwrap_or_default();
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(32)
        .collect();
    if cleaned.is_empty() {
        "user".to_owned()
    } else {
        cleaned
    }
}

/// The per-user directory endpoints live in: `%LOCALAPPDATA%\\Subordinate\\run`.
#[cfg(windows)]
fn runtime_directory() -> SubResult<PathBuf> {
    for key in ["LOCALAPPDATA", "TEMP", "TMP"] {
        if let Some(base) = non_empty_var(key) {
            return Ok(PathBuf::from(base).join("Subordinate").join("run"));
        }
    }
    Err(SubError::new(
        codes::ENDPOINT_UNAVAILABLE,
        "neither LOCALAPPDATA nor TEMP is set",
    ))
}

/// The per-user directory endpoints live in.
///
/// `TMPDIR` is already per-user on macOS (`/var/folders/…/T/`), which is where
/// Apple expects short-lived per-user sockets to go.
#[cfg(target_os = "macos")]
#[allow(clippy::unnecessary_wraps)]
fn runtime_directory() -> SubResult<PathBuf> {
    Ok(non_empty_var("TMPDIR").map_or_else(
        || PathBuf::from("/tmp").join(format!("subordinate-{}", current_user())),
        |base| PathBuf::from(base).join("subordinate"),
    ))
}

/// The per-user directory endpoints live in: `$XDG_RUNTIME_DIR/subordinate`,
/// falling back to a per-user directory under the temporary directory when the
/// session provides no runtime directory.
#[cfg(all(unix, not(target_os = "macos")))]
#[allow(clippy::unnecessary_wraps)]
fn runtime_directory() -> SubResult<PathBuf> {
    if let Some(base) = non_empty_var("XDG_RUNTIME_DIR") {
        return Ok(PathBuf::from(base).join("subordinate"));
    }
    let base = non_empty_var("TMPDIR").unwrap_or_else(|| "/tmp".to_owned());
    Ok(PathBuf::from(base).join(format!("subordinate-{}", current_user())))
}

/// An environment variable, treating an empty value as unset.
fn non_empty_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

/// Restricts `directory` to its owner where the platform has such a notion.
#[cfg(unix)]
fn private_directory(directory: &Path) -> SubResult<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).sub_context(
        codes::ENDPOINT_UNAVAILABLE,
        "the runtime directory could not be made private",
    )
}

/// Windows inherits the ACL of `%LOCALAPPDATA%`, which is already per-user.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn private_directory(_directory: &Path) -> SubResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Address, DEFAULT_INSTANCE, Endpoint, LockFile, MAX_UNIX_SOCKET_PATH, Transport,
        check_instance, current_user, fold,
    };
    use crate::codes;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sub-endpoint-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn instance_names_are_checked() {
        assert_eq!(check_instance("default").unwrap(), "default");
        assert_eq!(check_instance("a.b-c_1").unwrap(), "a.b-c_1");
        for bad in [
            "",
            ".",
            "..",
            "../escape",
            "a/b",
            "a b",
            "réel",
            &"x".repeat(65),
        ] {
            let error = check_instance(bad).unwrap_err();
            assert_eq!(error.code, codes::INVALID_INSTANCE, "{bad:?}");
        }
    }

    #[test]
    fn an_endpoint_names_a_socket_and_a_lock_file() {
        let directory = temp_dir("names");
        let endpoint = Endpoint::in_directory(&directory, DEFAULT_INSTANCE).unwrap();
        assert_eq!(endpoint.instance(), DEFAULT_INSTANCE);
        assert_eq!(endpoint.directory(), directory);
        assert_eq!(endpoint.lock_path(), directory.join("default.lock.json"));
        assert_eq!(endpoint.address().transport(), Transport::native());
        if cfg!(unix) {
            assert_eq!(
                endpoint.address().socket_path().unwrap(),
                directory.join("default.sock")
            );
        } else {
            assert!(endpoint.address().to_wire().starts_with(r"\\.\pipe\"));
        }
    }

    #[test]
    fn two_directories_do_not_share_an_address() {
        let one = Endpoint::in_directory(temp_dir("one"), "shared").unwrap();
        let two = Endpoint::in_directory(temp_dir("two"), "shared").unwrap();
        assert_ne!(one.address(), two.address());
    }

    #[test]
    fn the_user_default_lives_under_a_per_user_directory() {
        let endpoint = Endpoint::user_default().unwrap();
        let directory = endpoint.directory().to_string_lossy().into_owned();
        if cfg!(windows) {
            assert!(directory.contains("Subordinate"), "{directory}");
        } else {
            assert!(directory.contains("subordinate"), "{directory}");
            assert!(endpoint.address().to_wire().ends_with("default.sock"));
        }
    }

    #[test]
    fn an_over_long_socket_path_is_refused_on_unix() {
        let deep = std::path::PathBuf::from("/tmp").join("d".repeat(MAX_UNIX_SOCKET_PATH));
        let made = Endpoint::in_directory(deep, DEFAULT_INSTANCE);
        if cfg!(unix) {
            assert_eq!(
                made.unwrap_err().code,
                codes::ENDPOINT_UNAVAILABLE,
                "a path over the sun_path limit must be refused",
            );
        } else {
            assert!(made.is_ok(), "a pipe name does not depend on the directory");
        }
    }

    #[test]
    fn an_address_survives_the_lock_file_round_trip() {
        for address in [
            Address::UnixSocket("/run/user/1000/subordinate/default.sock".into()),
            Address::WindowsNamedPipe("subordinate.tester.default".to_owned()),
        ] {
            let wire = address.to_wire();
            let back = Address::from_wire(address.transport(), &wire).unwrap();
            assert_eq!(back, address, "{wire}");
        }
    }

    #[test]
    fn a_pipe_path_without_the_prefix_is_refused() {
        let error = Address::from_wire(Transport::WindowsNamedPipe, "subordinate.x").unwrap_err();
        assert_eq!(error.code, codes::LOCK_FILE_INVALID);
        let error = Address::from_wire(Transport::UnixSocket, "").unwrap_err();
        assert_eq!(error.code, codes::LOCK_FILE_INVALID);
    }

    #[test]
    fn a_lock_file_is_written_read_and_validated() {
        let directory = temp_dir("lock");
        let endpoint = Endpoint::in_directory(&directory, "lockable").unwrap();
        endpoint.create_directory().unwrap();
        assert_eq!(endpoint.read_lock().unwrap(), None);

        let lock = LockFile::for_endpoint(&endpoint);
        lock.write(&endpoint.lock_path()).unwrap();
        let read = endpoint.read_lock().unwrap().unwrap();
        assert_eq!(read, lock);
        assert_eq!(read.pid, std::process::id());
        assert_eq!(read.address().unwrap(), *endpoint.address());

        std::fs::write(endpoint.lock_path(), "not json").unwrap();
        assert_eq!(
            endpoint.read_lock().unwrap_err().code,
            codes::LOCK_FILE_INVALID,
        );
    }

    #[test]
    fn the_directory_is_private_to_this_user() {
        let directory = temp_dir("private").join("nested");
        let endpoint = Endpoint::in_directory(&directory, DEFAULT_INSTANCE).unwrap();
        endpoint.create_directory().unwrap();
        assert!(directory.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&directory).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{mode:o}");
        }
    }

    #[test]
    fn a_folded_path_is_stable_and_a_user_name_is_safe() {
        assert_eq!(
            fold(std::path::Path::new("/tmp/a")),
            fold("/tmp/a".as_ref())
        );
        assert_ne!(fold("/tmp/a".as_ref()), fold("/tmp/b".as_ref()));
        let user = current_user();
        assert!(!user.is_empty());
        assert!(
            user.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
            "{user}",
        );
    }
}
