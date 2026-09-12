//! The editor's own Command API endpoint: the socket the MCP bridge drives.
//!
//! One Command API serves the GUI, the CLI, the MCP bridge and plugins
//! (docs/PLAN.md §4 and §7). The in-process half of that has always been here —
//! [`EditorSession`] owns a [`Dispatcher`] over the engine the panels draw —
//! and this is the other half: a [`Server`] bound on the per-user endpoint, so
//! an agent's edit lands in the window the user is looking at instead of in a
//! headless engine of its own (TASK-137's finding, TASK-141).
//!
//! Three rules shape it.
//!
//! - **Never on the UI thread.** Binding touches the filesystem and, when a
//!   lock file is already there, tries to connect to whatever wrote it; that is
//!   milliseconds at best and a connect timeout at worst. So the bind runs on a
//!   thread of its own and the outcome is collected at the next
//!   [`CommandApi::sync`], a frame or two later. Everything after it is off the
//!   UI thread too: the server accepts on its own thread, serves each client on
//!   another, and every command those threads dispatch is queued to the engine
//!   thread like any other. The UI thread's only part is the once-a-frame
//!   [`EditorSession::poll`] that turns the resulting change events into a
//!   repaint.
//! - **The socket fronts the engine the panels are drawing.** Opening a project
//!   replaces the engine and the dispatcher with it, so the server is rebound
//!   whenever [`EditorSession::generation`] moves. Commands that arrive over
//!   the socket therefore go through the session's own dispatcher: they enter
//!   the same undo history the panels write to, and the panels see them on the
//!   next frame through the change events they already read.
//! - **One editor to an endpoint.** See [`CommandApi::serve`] for the rule.
//!
//! The endpoint goes away with the editor: [`Server`] removes its socket and
//! its lock file as it drops, and [`CommandApi::shutdown`] is called from the
//! window's exit path so that a closed editor never leaves a lock file for the
//! next one to clean up.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;

use sub_command::codes;
use sub_command::endpoint::{Address, Endpoint};
use sub_command::transport::Server;
use sub_core::{SubError, SubResult};

use crate::session::EditorSession;

/// The editor's endpoint, the server on it, and the bind in flight.
#[derive(Debug)]
pub struct CommandApi {
    /// Where this editor listens.
    endpoint: Endpoint,
    /// The running server, once one is bound.
    server: Option<Server>,
    /// The bind that has been started and not yet collected.
    binding: Option<Binding>,
    /// The session generation the server — or the bind in flight — belongs to.
    ///
    /// `None` until the first bind is started, which is what makes the first
    /// [`CommandApi::sync`] bind rather than wait for a project to be opened.
    generation: Option<u64>,
    /// Why the last bind was refused, when it was.
    refusal: Option<SubError>,
}

/// A bind running on its own thread.
#[derive(Debug)]
struct Binding {
    /// The session generation being bound for.
    generation: u64,
    /// Where the binding thread posts its result.
    outcome: Receiver<SubResult<Server>>,
}

impl CommandApi {
    /// An endpoint that is not being served yet.
    #[must_use]
    pub const fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            server: None,
            binding: None,
            generation: None,
            refusal: None,
        }
    }

    /// Where this editor listens, whether or not it holds the endpoint.
    #[must_use]
    pub const fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The address clients reach this editor on, once it is serving.
    #[must_use]
    pub fn address(&self) -> Option<&Address> {
        self.server
            .as_ref()
            .map(|server| server.endpoint().address())
    }

    /// Whether the endpoint is bound right now.
    #[must_use]
    pub const fn is_serving(&self) -> bool {
        self.server.is_some()
    }

    /// Why this editor is not serving, when a bind was refused.
    #[must_use]
    pub const fn refusal(&self) -> Option<&SubError> {
        self.refusal.as_ref()
    }

    /// One line for the status bar and the log: what the agent surface is
    /// doing.
    #[must_use]
    pub fn status_line(&self) -> String {
        if let Some(address) = self.address() {
            return format!("Command API listening on {}", address.to_wire());
        }
        if let Some(refusal) = &self.refusal {
            return format!("Command API not served: {}", refusal.message);
        }
        if self.binding.is_some() {
            return "Command API starting".to_owned();
        }
        "Command API not served".to_owned()
    }

    /// Collects a finished bind and starts one when the session's engine has
    /// been replaced.
    ///
    /// Called once a frame. It never blocks: a bind still running is left for
    /// the next frame.
    pub fn sync(&mut self, session: &EditorSession) {
        self.collect();
        if self.binding.is_none() && self.generation != Some(session.generation()) {
            self.serve(session);
        }
    }

    /// Binds the endpoint for `session`, replacing whatever was being served.
    ///
    /// **The single-instance rule.** The endpoint belongs to whoever is
    /// answering on it. A bind first asks the address and the one the lock file
    /// records whether anything replies
    /// (`sub_command::transport::clear_stale`):
    ///
    /// - Something replies: another editor is live, this one *refuses* to take
    ///   the endpoint and keeps running with no agent surface of its own. The
    ///   refusal is `command.address_in_use`, it is logged with the other
    ///   process's pid, and [`CommandApi::refusal`] carries it so the window
    ///   can say so. A second editor is a perfectly good editor; it simply is
    ///   not the one an agent reaches, and the first one keeps the clients it
    ///   already has. Starting it with a different instance name gives it an
    ///   endpoint of its own.
    /// - Nothing replies: the socket and the lock file are a corpse left by an
    ///   editor that crashed, so this one *takes over*, unlinks them and binds.
    ///
    /// A refusal is not retried every frame — a live neighbour would make that
    /// a spin — but the next bind, which is to say the next project opened,
    /// tries again.
    pub fn serve(&mut self, session: &EditorSession) {
        // The address has to be free before the new listener asks for it, and
        // dropping the server is what unlinks the socket and the lock file.
        self.stop();
        let generation = session.generation();
        let dispatcher = session.commands_arc();
        let endpoint = self.endpoint.clone();
        let (sender, outcome) = channel();
        let started = thread::Builder::new()
            .name("sub-ui-command-api".to_owned())
            .spawn(move || {
                // A receiver dropped before the bind finishes means the window
                // is already closing; the server then drops here, which
                // releases the endpoint again.
                let _ = sender.send(Server::bind(endpoint, dispatcher));
            });
        match started {
            Ok(_handle) => {
                self.generation = Some(generation);
                self.binding = Some(Binding {
                    generation,
                    outcome,
                });
            }
            Err(error) => {
                self.generation = Some(generation);
                self.refuse(
                    SubError::new(
                        codes::TRANSPORT_IO,
                        "the Command API listener could not be started",
                    )
                    .with_cause(&error),
                );
            }
        }
    }

    /// Stops serving and releases the endpoint.
    ///
    /// A bind still in flight is abandoned rather than waited for: its thread
    /// finds the channel closed, drops the server it made, and the endpoint
    /// goes with it.
    pub fn shutdown(&mut self) {
        self.stop();
        self.binding = None;
    }

    /// Takes the result of a finished bind.
    fn collect(&mut self) {
        let Some(binding) = &self.binding else {
            return;
        };
        let outcome = match binding.outcome.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(SubError::new(
                codes::TRANSPORT_IO,
                "the Command API listener stopped before it was bound",
            )),
        };
        let generation = binding.generation;
        self.binding = None;
        match outcome {
            Ok(server) => {
                log::info!(
                    "the Command API is listening on {} (lock file {})",
                    server.endpoint().address().to_wire(),
                    server.endpoint().lock_path().display(),
                );
                self.server = Some(server);
                self.refusal = None;
                self.generation = Some(generation);
            }
            Err(error) => {
                self.generation = Some(generation);
                self.refuse(error);
            }
        }
    }

    /// Records a refusal and says so once.
    fn refuse(&mut self, error: SubError) {
        if error.code == codes::ADDRESS_IN_USE {
            log::warn!(
                "another editor is already serving {}; this window is not the one agents reach: \
                 [{}] {}",
                self.endpoint.address().to_wire(),
                error.code,
                error.message,
            );
        } else {
            log::warn!(
                "the Command API is not served on {}: [{}] {}",
                self.endpoint.address().to_wire(),
                error.code,
                error.message,
            );
        }
        self.refusal = Some(error);
    }

    /// Drops the running server, which releases the address and removes the
    /// lock file.
    fn stop(&mut self) {
        if let Some(server) = self.server.take()
            && let Err(error) = server.shutdown()
        {
            log::warn!(
                "the Command API endpoint was not cleaned up: [{}] {}",
                error.code,
                error.message,
            );
        }
    }
}

impl Drop for CommandApi {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::CommandApi;
    use crate::session::EditorSession;
    use std::path::PathBuf;
    use sub_command::endpoint::Endpoint;
    use sub_command::transport::Client;
    use sub_model::{Project, Sequence, sequence::SequenceSettings};

    /// A folder of this test's own, emptied first so a rerun starts clean.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sub-ui-command-api-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary folder");
        dir
    }

    /// A project with one sequence, so a command has something to name.
    fn project() -> Project {
        let mut project = Project::new("Doc cut");
        project
            .sequences
            .push(Sequence::new("Main", SequenceSettings::default()));
        project
    }

    /// Drives `api` until it has settled, which takes as long as one thread
    /// takes to bind a socket.
    fn settle(api: &mut CommandApi, session: &EditorSession) {
        for _ in 0..200 {
            api.sync(session);
            if api.is_serving() || api.refusal().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the Command API neither bound nor refused");
    }

    #[test]
    fn the_endpoint_serves_the_session_and_goes_away_with_it() {
        let dir = temp_dir("serves");
        let endpoint = Endpoint::in_directory(&dir, "serves").expect("an endpoint");
        let lock = endpoint.lock_path();
        let session = EditorSession::new(project()).expect("the engine starts");
        let mut api = CommandApi::new(endpoint);
        settle(&mut api, &session);
        assert!(api.is_serving(), "{:?}", api.refusal());
        assert!(lock.is_file(), "a serving editor publishes its lock file");

        let mut client = Client::connect(api.endpoint()).expect("a client connects");
        let revision = client
            .invoke("project.revision", None)
            .expect("project.revision answers");
        assert_eq!(revision["revision"], 0);
        drop(client);

        api.shutdown();
        assert!(!api.is_serving());
        assert!(!lock.exists(), "a closed editor leaves no lock file");
    }

    #[test]
    fn a_second_editor_refuses_the_endpoint_and_keeps_running() {
        let dir = temp_dir("second");
        let first_session = EditorSession::new(project()).expect("the engine starts");
        let mut first = CommandApi::new(Endpoint::in_directory(&dir, "second").expect("endpoint"));
        settle(&mut first, &first_session);
        assert!(first.is_serving(), "{:?}", first.refusal());

        let second_session = EditorSession::new(project()).expect("the engine starts");
        let mut second = CommandApi::new(Endpoint::in_directory(&dir, "second").expect("endpoint"));
        settle(&mut second, &second_session);
        assert!(!second.is_serving(), "the endpoint is taken");
        assert_eq!(
            second.refusal().expect("a refusal").code.as_str(),
            "command.address_in_use",
        );
        assert!(
            first.is_serving(),
            "the editor that holds the endpoint keeps it",
        );
    }

    #[test]
    fn a_stale_lock_file_is_taken_over() {
        let dir = temp_dir("stale");
        let endpoint = Endpoint::in_directory(&dir, "stale").expect("an endpoint");
        endpoint.create_directory().expect("the directory");
        // What a crashed editor leaves: a lock file naming a socket nothing is
        // listening on.
        sub_command::LockFile::for_endpoint(&endpoint)
            .write(&endpoint.lock_path())
            .expect("the corpse writes");

        let session = EditorSession::new(project()).expect("the engine starts");
        let mut api = CommandApi::new(endpoint);
        settle(&mut api, &session);
        assert!(
            api.is_serving(),
            "a dead instance's lock file is not a live one: {:?}",
            api.refusal(),
        );
    }

    #[test]
    fn the_socket_follows_the_project_that_is_open() {
        let dir = temp_dir("rebind");
        let file = dir.join("cut.sub");
        std::fs::write(
            &file,
            sub_model::json::to_json(&project()).expect("the project serialises"),
        )
        .expect("the fixture writes");

        let mut session = EditorSession::new(Project::new("Untitled")).expect("the engine starts");
        let mut api = CommandApi::new(Endpoint::in_directory(&dir, "rebind").expect("endpoint"));
        settle(&mut api, &session);
        assert!(api.is_serving(), "{:?}", api.refusal());

        // Opening a project replaces the engine; the socket has to follow it,
        // or an agent would be editing a project nobody is looking at.
        session.open(&file).expect("the project opens");
        api.sync(&session);
        settle(&mut api, &session);
        assert!(api.is_serving(), "{:?}", api.refusal());

        let mut client = Client::connect(api.endpoint()).expect("a client connects");
        let answer = client
            .invoke("project.get", None)
            .expect("project.get answers");
        // `project.get` answers with the project file document, so the
        // project itself is one level in.
        assert_eq!(answer["project"]["project"]["name"], "Doc cut");
    }
}
