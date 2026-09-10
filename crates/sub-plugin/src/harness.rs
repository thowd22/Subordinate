//! The headless test harness: proving a plugin works without an editor.
//!
//! An agent that has just written a plugin needs an answer to one question —
//! does it work? — and it needs that answer as data, not as a screenshot
//! (docs/PLAN.md §6.4). This module is the machinery behind
//! `subordinate-cli plugin test <id>`: it loads a component, serves it the
//! real Command API over a fixture project, exercises every world its
//! `plugin.toml` declares, and reports what happened as a [`TestReport`] of
//! individually named checks.
//!
//! What "works" means depends on the world:
//!
//! - **command** — the plugin is run against the fixture and the project state
//!   is asserted afterwards: the run must answer JSON, and whatever it changed
//!   must be undoable, because a plugin edit is an ordinary command on the
//!   host's undo stack and nothing else.
//! - **effect** — the declaration is lifted into the compositor's own types
//!   and a frame is rendered with it bound at its defaults, so a shader that
//!   does not compile or an entry point that is not there fails the test run
//!   rather than the editor's next composite.
//! - **analyzer** — `analyze` is run over the fixture's media and its findings
//!   are lifted into the model, so a marker at an impossible time is caught
//!   here.
//! - **mcp-tools** — the exported tools are checked against the manifest and
//!   each one is called with arguments its own schema accepts.
//!
//! Every check carries a [`CheckStatus`]. A failure is never a panic and
//! rarely an `Err`: a plugin that traps, runs out of fuel or returns an error
//! is *reported*, with its stable code and hint, as a failed check beside the
//! ones that passed — which is what makes the report worth printing whichever
//! way the run went. [`TestReport::ok`] is what a caller turns into an exit
//! code.
//!
//! The host the plugin sees here is the real one: [`command-api`](crate)
//! calls dispatch into a [`Dispatcher`] over a live [`EngineHandle`], so a
//! plugin that edits the fixture edits it through the same undoable commands
//! the GUI uses, and the accessors answer out of the engine's own snapshot.
//! Nothing is granted that a manifest did not ask for: the sandbox opens no
//! preopened directory and no socket.

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value, json};
use sub_command::Dispatcher;
use sub_core::{SubError, SubResult, codes as core_codes};
use sub_edit::EngineHandle;
use sub_model::ProjectId;
use wasmtime::StoreLimits;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::command_api::{
    ClipMetadata, Host, LogLevel, MarkerMetadata, ProjectMetadata, SequenceMetadata, TrackMetadata,
};
use crate::manifest::{Manifest, PluginId, World};
use crate::mcp::ToolCatalog;
use crate::runtime::{Limits, PluginRuntime, PluginState, termination};
use crate::{
    AnalyzerWorld, Command, Effect, McpTools, WitError, WitProjectId, WitRationalTime,
    WitSequenceId, WitTrackId, analysis_from_wit, codes, marker_metadata, project_metadata,
    sequence_metadata, track_clip_metadata, track_metadata,
};

/// The arguments a test run hands a plugin when the caller names none.
pub const DEFAULT_ARGS: &str = "{}";

/// How one check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// The plugin did what the world's contract says it should.
    Pass,
    /// It did not, and the check carries why.
    Fail,
    /// The check could not run here — no GPU, a tool whose schema wants
    /// arguments the harness cannot invent — and says so rather than
    /// pretending either answer.
    Skip,
}

impl CheckStatus {
    /// The word a report prints for this status.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }
}

/// One named thing the harness asked of a plugin, and what came back.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// The WIT world the check belongs to, or `plugin` for the ones that come
    /// before any world does.
    pub world: String,
    /// What was checked, as a stable `snake_case` name.
    pub name: String,
    /// Whether it passed.
    pub status: CheckStatus,
    /// One line a person reads.
    pub message: String,
    /// Whatever the check learned: counts, the answer a call returned, the
    /// pixel a frame came back as.
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub detail: Map<String, Value>,
    /// The structured error behind a failure, with its stable code, the WIT
    /// item it belongs to and a hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

impl Check {
    /// A check that passed.
    fn pass(world: World, name: &str, message: impl Into<String>) -> Self {
        Self::new(world.as_str(), name, CheckStatus::Pass, message)
    }

    /// A check that failed for a reason with no [`SubError`] behind it.
    fn fail(world: World, name: &str, message: impl Into<String>) -> Self {
        Self::new(world.as_str(), name, CheckStatus::Fail, message)
    }

    /// A check that could not run.
    fn skip(world: World, name: &str, message: impl Into<String>) -> Self {
        Self::new(world.as_str(), name, CheckStatus::Skip, message)
    }

    /// A check that failed because a call returned a structured error.
    fn failed_with(world: World, name: &str, error: &SubError) -> Self {
        Self::new(
            world.as_str(),
            name,
            CheckStatus::Fail,
            error.message.clone(),
        )
        .with_error(error)
    }

    /// The general constructor the four above share.
    fn new(world: &str, name: &str, status: CheckStatus, message: impl Into<String>) -> Self {
        Self {
            world: world.to_owned(),
            name: name.to_owned(),
            status,
            message: message.into(),
            detail: Map::new(),
            error: None,
        }
    }

    /// The same check carrying one more detail.
    #[must_use]
    fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.detail.insert(key.to_owned(), value.into());
        self
    }

    /// The same check carrying the error behind it.
    #[must_use]
    fn with_error(mut self, error: &SubError) -> Self {
        self.error = Some(error.to_json());
        self
    }
}

/// One line a plugin wrote to the host's log during the run.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    /// The level the plugin asked for.
    pub level: String,
    /// The message it wrote.
    pub message: String,
}

/// What a whole test run came to.
#[derive(Debug, Clone, Serialize)]
pub struct TestReport {
    /// The plugin that was tested.
    pub plugin: String,
    /// The worlds its manifest declares, in manifest order.
    pub worlds: Vec<String>,
    /// Every check, in the order they ran.
    pub checks: Vec<Check>,
    /// Everything the plugin logged while it ran.
    pub log: Vec<LogLine>,
}

impl TestReport {
    /// An empty report for `plugin`.
    #[must_use]
    pub fn new(plugin: &PluginId, worlds: &[World]) -> Self {
        Self {
            plugin: plugin.as_str().to_owned(),
            worlds: worlds.iter().map(ToString::to_string).collect(),
            checks: Vec::new(),
            log: Vec::new(),
        }
    }

    /// How many checks passed.
    #[must_use]
    pub fn passed(&self) -> usize {
        self.count(CheckStatus::Pass)
    }

    /// How many failed.
    #[must_use]
    pub fn failed(&self) -> usize {
        self.count(CheckStatus::Fail)
    }

    /// How many could not run here.
    #[must_use]
    pub fn skipped(&self) -> usize {
        self.count(CheckStatus::Skip)
    }

    /// How many checks ended with `status`.
    fn count(&self, status: CheckStatus) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == status)
            .count()
    }

    /// Whether the run proved the plugin works: at least one check ran and
    /// none failed.
    ///
    /// A report with nothing but skips is not a pass: a plugin whose only
    /// world could not be exercised here has not been shown to work.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.failed() == 0 && self.passed() > 0
    }

    /// The one line a person reads first.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{}: {} passed, {} failed, {} skipped",
            self.plugin,
            self.passed(),
            self.failed(),
            self.skipped(),
        )
    }

    /// The report as the JSON the CLI and the MCP bridge print, with the
    /// counts and the verdict alongside the checks.
    ///
    /// # Errors
    ///
    /// [`sub_core::codes::INTERNAL`] if the report cannot be serialised, which
    /// would be a bug here rather than something a caller can fix.
    pub fn to_json(&self) -> SubResult<Value> {
        let mut value = serde_json::to_value(self).map_err(|err| {
            SubError::wrap(
                core_codes::INTERNAL,
                "the plugin test report could not be serialised",
                &err,
            )
        })?;
        value["passed"] = json!(self.passed());
        value["failed"] = json!(self.failed());
        value["skipped"] = json!(self.skipped());
        value["ok"] = json!(self.ok());
        value["summary"] = json!(self.summary());
        Ok(value)
    }

    /// Adds one check.
    fn push(&mut self, check: Check) {
        self.checks.push(check);
    }
}

/// The state one plugin instance sees while it is being tested.
///
/// It is the real host: `run-command` and `query` go through the
/// [`Dispatcher`], so a plugin's edit is an ordinary undoable command, and the
/// accessors answer out of the engine's snapshot rather than a fixture of
/// their own. The WASI context grants nothing — no preopened directory, no
/// socket, no environment — because a capability nobody approved is a denied
/// one.
pub struct HarnessHost {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    engine: EngineHandle,
    dispatcher: Arc<Dispatcher>,
    project: ProjectId,
    log: Vec<LogLine>,
    progress: Vec<(u64, u64)>,
}

impl HarnessHost {
    /// A host over `engine`, dispatching through `dispatcher`.
    fn new(engine: EngineHandle, dispatcher: Arc<Dispatcher>) -> Self {
        let project = engine.snapshot().id;
        Self {
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
            limits: StoreLimits::default(),
            engine,
            dispatcher,
            project,
            log: Vec::new(),
            progress: Vec::new(),
        }
    }

    /// Everything the plugin logged, in order.
    #[must_use]
    pub fn log(&self) -> &[LogLine] {
        &self.log
    }

    /// The progress an analyzer reported, as `(done, total)` pairs.
    #[must_use]
    pub fn progress(&self) -> &[(u64, u64)] {
        &self.progress
    }

    /// Rejects a call naming a project this host does not have open.
    fn check(&self, project: &WitProjectId) -> Result<(), WitError> {
        let id = ProjectId::try_from(project).map_err(WitError::from)?;
        if id == self.project {
            return Ok(());
        }
        Err(SubError::new(NO_SUCH_PROJECT, "no such project")
            .with_detail("project", project.value.clone())
            .into())
    }

    /// One sequence out of the current snapshot, by identifier.
    fn dispatch(&self, method: &str, params: &str) -> Result<String, WitError> {
        let params: Value = serde_json::from_str(params).map_err(|err| {
            WitError::from(
                SubError::wrap(
                    core_codes::INVALID_ARGUMENT,
                    "a Command API call's params must be a JSON object",
                    &err,
                )
                .with_detail("method", method.to_owned()),
            )
        })?;
        let answer = self
            .dispatcher
            .invoke(method, Some(params))
            .map_err(|error| WitError::from(&error))?;
        serde_json::to_string(&answer).map_err(|err| {
            WitError::from(SubError::wrap(
                core_codes::INTERNAL,
                "a Command API answer could not be serialised",
                &err,
            ))
        })
    }
}

/// The code a call naming an unopened project comes back with, which is the
/// one the editor's own host uses.
const NO_SUCH_PROJECT: sub_core::ErrorCode =
    sub_core::ErrorCode::from_static("plugin.no_such_project");

/// The code an accessor uses for a sequence or track the project does not
/// hold.
const NO_SUCH_SEQUENCE: sub_core::ErrorCode =
    sub_core::ErrorCode::from_static("plugin.no_such_sequence");

/// The same, for a track.
const NO_SUCH_TRACK: sub_core::ErrorCode = sub_core::ErrorCode::from_static("plugin.no_such_track");

impl PluginState for HarnessHost {
    fn store_limits(&mut self) -> &mut StoreLimits {
        &mut self.limits
    }
}

impl WasiView for HarnessHost {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl crate::types::Host for HarnessHost {}
impl crate::effect_types::Host for HarnessHost {}
impl crate::command_menu::Host for HarnessHost {}
impl crate::mcp_types::Host for HarnessHost {}
impl crate::audio_types::Host for HarnessHost {}
impl crate::analysis::Host for HarnessHost {}

impl crate::analysis_host::Host for HarnessHost {
    fn report_progress(&mut self, done: u64, total: u64) {
        self.progress.push((done, total));
    }

    fn is_cancelled(&mut self) -> bool {
        // A test run is never cancelled: an analyzer that asks is told to
        // carry on, so the whole of its work is what the report describes.
        false
    }
}

impl Host for HarnessHost {
    fn run_command(
        &mut self,
        project: WitProjectId,
        method: String,
        params: String,
    ) -> Result<String, WitError> {
        self.check(&project)?;
        self.dispatch(&method, &params)
    }

    fn query(
        &mut self,
        project: WitProjectId,
        method: String,
        params: String,
    ) -> Result<String, WitError> {
        self.check(&project)?;
        self.dispatch(&method, &params)
    }

    fn open_projects(&mut self) -> Vec<WitProjectId> {
        vec![self.project.into()]
    }

    fn project_info(&mut self, project: WitProjectId) -> Result<ProjectMetadata, WitError> {
        self.check(&project)?;
        let snapshot = self.engine.snapshot();
        Ok(project_metadata(&snapshot, self.engine.revision()))
    }

    fn sequences(&mut self, project: WitProjectId) -> Result<Vec<SequenceMetadata>, WitError> {
        self.check(&project)?;
        Ok(self
            .engine
            .snapshot()
            .sequences
            .iter()
            .map(sequence_metadata)
            .collect())
    }

    fn tracks(
        &mut self,
        project: WitProjectId,
        sequence: WitSequenceId,
    ) -> Result<Vec<TrackMetadata>, WitError> {
        self.check(&project)?;
        let id = sub_model::SequenceId::try_from(&sequence).map_err(WitError::from)?;
        let snapshot = self.engine.snapshot();
        let sequence = snapshot.sequence(id).ok_or_else(no_such_sequence)?;
        let rate = sequence.settings.frame_rate;
        Ok(sequence
            .tracks
            .iter()
            .map(|track| track_metadata(track, rate))
            .collect())
    }

    fn clips(
        &mut self,
        project: WitProjectId,
        sequence: WitSequenceId,
        track: WitTrackId,
    ) -> Result<Vec<ClipMetadata>, WitError> {
        self.check(&project)?;
        let sequence_id = sub_model::SequenceId::try_from(&sequence).map_err(WitError::from)?;
        let track_id = sub_model::TrackId::try_from(&track).map_err(WitError::from)?;
        let snapshot = self.engine.snapshot();
        let sequence = snapshot
            .sequence(sequence_id)
            .ok_or_else(no_such_sequence)?;
        let track = sequence.track(track_id).ok_or_else(|| {
            WitError::from(SubError::new(
                NO_SUCH_TRACK,
                "no such track in this sequence",
            ))
        })?;
        Ok(track_clip_metadata(track, sequence.settings.frame_rate))
    }

    fn markers(
        &mut self,
        project: WitProjectId,
        sequence: WitSequenceId,
    ) -> Result<Vec<MarkerMetadata>, WitError> {
        self.check(&project)?;
        let id = sub_model::SequenceId::try_from(&sequence).map_err(WitError::from)?;
        let snapshot = self.engine.snapshot();
        let sequence = snapshot.sequence(id).ok_or_else(no_such_sequence)?;
        Ok(sequence.markers.iter().map(marker_metadata).collect())
    }

    fn playhead(&mut self, project: WitProjectId) -> Result<WitRationalTime, WitError> {
        self.check(&project)?;
        let status = self
            .engine
            .playback_status()
            .map_err(|error| WitError::from(&error))?;
        Ok(status.position.into())
    }

    fn log(&mut self, level: LogLevel, message: String) {
        let level = match level {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        };
        tracing::debug!(target: "plugin", level, "{message}");
        self.log.push(LogLine {
            level: level.to_owned(),
            message,
        });
    }
}

/// The error an accessor answers for a sequence the project does not hold.
fn no_such_sequence() -> WitError {
    WitError::from(SubError::new(
        NO_SUCH_SEQUENCE,
        "no such sequence in this project",
    ))
}

/// A plugin under test: one component, one fixture project, one report.
///
/// The engine and the dispatcher are the caller's: the CLI opens the fixture,
/// spawns the engine and registers the same method table the socket serves, so
/// what a plugin can reach here is exactly what it can reach in the editor.
pub struct Harness {
    runtime: PluginRuntime,
    engine: EngineHandle,
    dispatcher: Arc<Dispatcher>,
    limits: Limits,
    args: String,
}

impl std::fmt::Debug for Harness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Harness")
            .field("limits", &self.limits)
            .field("args", &self.args)
            .finish_non_exhaustive()
    }
}

impl Harness {
    /// Builds a harness over a live engine and the dispatcher serving it.
    ///
    /// # Errors
    ///
    /// [`codes::ENGINE_FAILED`] when this build has no wasm compiler, which is
    /// the one failure that stops a test run before any plugin is loaded.
    pub fn new(engine: EngineHandle, dispatcher: Arc<Dispatcher>) -> SubResult<Self> {
        Ok(Self {
            runtime: PluginRuntime::new()?,
            engine,
            dispatcher,
            limits: Limits::default(),
            args: DEFAULT_ARGS.to_owned(),
        })
    }

    /// The same harness running plugins under `limits` instead of the
    /// defaults.
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The same harness handing a command plugin `args` instead of `{}`.
    #[must_use]
    pub fn with_args(mut self, args: impl Into<String>) -> Self {
        self.args = args.into();
        self
    }

    /// Loads `wasm` and runs every check its manifest's worlds call for.
    ///
    /// A component that will not compile is reported as a failed
    /// `component_loads` check rather than an `Err`, so the caller has a whole
    /// report to print either way.
    ///
    /// # Errors
    ///
    /// Only what the engine itself returns: a run whose undo group cannot be
    /// opened or closed. Everything a plugin can do wrong is a failed check.
    pub fn run(
        &self,
        plugin: &PluginId,
        manifest: &Manifest,
        directory: &Path,
        wasm: &Path,
    ) -> SubResult<TestReport> {
        let mut report = TestReport::new(plugin, &manifest.plugin.worlds);
        let component = match self.runtime.compile_file(plugin, wasm) {
            Ok(component) => component,
            Err(error) => {
                let error = crate::errors::explain(error);
                report.push(
                    Check::new(
                        "plugin",
                        "component_loads",
                        CheckStatus::Fail,
                        error.message.clone(),
                    )
                    .with_error(&error),
                );
                return Ok(report);
            }
        };
        report.push(
            Check::new(
                "plugin",
                "component_loads",
                CheckStatus::Pass,
                "the component compiled",
            )
            .with("path", wasm.display().to_string()),
        );

        for world in &manifest.plugin.worlds {
            match world {
                World::Command => self.check_command(plugin, &component, &mut report)?,
                World::Effect => self.check_effect(plugin, &component, &mut report),
                World::Analyzer => self.check_analyzer(plugin, &component, &mut report),
                World::McpTools => {
                    self.check_mcp_tools(plugin, manifest, directory, &component, &mut report);
                }
                other => {
                    report.push(Check::skip(
                        *other,
                        "world_supported",
                        format!("the harness runs no checks for the {other} world yet"),
                    ));
                }
            }
        }
        Ok(report)
    }

    /// A store and a linker for one world.
    fn store(&self) -> SubResult<wasmtime::Store<HarnessHost>> {
        self.runtime.store(
            HarnessHost::new(self.engine.clone(), Arc::clone(&self.dispatcher)),
            self.limits,
        )
    }

    /// The empty linker every world starts from: WASI and nothing else.
    fn linker(&self) -> SubResult<Linker<HarnessHost>> {
        let mut linker = Linker::<HarnessHost>::new(self.runtime.engine());
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|err| link_failed(&err))?;
        Ok(linker)
    }

    /// The `command` world: run it against the fixture and assert the project
    /// state afterwards.
    fn check_command(
        &self,
        plugin: &PluginId,
        component: &Component,
        report: &mut TestReport,
    ) -> SubResult<()> {
        let world = World::Command;
        let mut linker = match self.linker() {
            Ok(linker) => linker,
            Err(error) => {
                report.push(Check::failed_with(world, "links", &error));
                return Ok(());
            }
        };
        if let Err(error) =
            Command::add_to_linker::<_, HasSelf<HarnessHost>>(&mut linker, |state| state)
                .map_err(|err| link_failed(&err))
        {
            report.push(Check::failed_with(world, "links", &error));
            return Ok(());
        }
        let mut store = match self.store() {
            Ok(store) => store,
            Err(error) => {
                report.push(Check::failed_with(world, "instantiates", &error));
                return Ok(());
            }
        };
        let instance = match Command::instantiate(&mut store, component, &linker) {
            Ok(instance) => instance,
            Err(err) => {
                let error = instantiate_failed(plugin, &err);
                report.push(Check::failed_with(world, "instantiates", &error));
                return Ok(());
            }
        };
        report.push(Check::pass(
            world,
            "instantiates",
            "the component instantiated with every import satisfied",
        ));

        let before = self.state()?;
        let project: WitProjectId = self.engine.snapshot().id.into();

        // The host opens one undo group around a plugin run, so however many
        // primitives the plugin applies, the fixture sees one step.
        self.engine.begin_group(format!("Plugin test {plugin}"))?;
        let answer = instance.call_run(&mut store, &project, &self.args);
        let answer = match answer {
            Ok(Ok(answer)) => {
                self.engine.commit_group()?;
                Ok(answer)
            }
            Ok(Err(err)) => {
                drop(self.engine.abort_group());
                Err(crate::errors::explain(SubError::from(err)))
            }
            Err(err) => {
                drop(self.engine.abort_group());
                Err(crate::errors::explain(termination(plugin, &err)))
            }
        };
        report.log.extend(store.data().log().iter().cloned());

        let answer = match answer {
            Ok(answer) => {
                report.push(
                    Check::pass(world, "run", "run returned")
                        .with("answer_bytes", answer.len())
                        .with("args", self.args.clone()),
                );
                answer
            }
            Err(error) => {
                report.push(Check::failed_with(world, "run", &error));
                return Ok(());
            }
        };

        match serde_json::from_str::<Value>(&answer) {
            Ok(value) => report.push(
                Check::pass(world, "answers_json", "run answered a JSON document")
                    .with("answer", value),
            ),
            Err(err) => report.push(
                Check::fail(
                    world,
                    "answers_json",
                    format!("run answered something that is not JSON: {err}"),
                )
                .with("answer", answer),
            ),
        }

        self.assert_project_state(&before, report)
    }

    /// The half of the `command` world's contract that is about the project:
    /// what the run changed, and whether it undoes.
    fn assert_project_state(
        &self,
        before: &ProjectState,
        report: &mut TestReport,
    ) -> SubResult<()> {
        let world = World::Command;
        let after = self.state()?;
        let changed = after != *before;
        report.push(
            Check::pass(
                world,
                "project_state",
                if changed {
                    "the project changed while the plugin ran"
                } else {
                    "the plugin left the project as it found it"
                },
            )
            .with("before", state_json(before))
            .with("after", state_json(&after))
            .with("changed", changed),
        );

        if !changed {
            report.push(Check::skip(
                world,
                "undoable",
                "the run changed nothing, so there was nothing to undo",
            ));
            return Ok(());
        }

        let undone = self.engine.undo()?;
        let restored = self.state()?;
        if undone.is_some() && restored == *before {
            report.push(Check::pass(
                world,
                "undoable",
                "undo put the project back exactly as it was",
            ));
        } else {
            report.push(
                Check::fail(
                    world,
                    "undoable",
                    "the plugin's changes did not undo cleanly; every edit must go through the \
                     Command API",
                )
                .with("after_undo", state_json(&restored)),
            );
        }
        // Leave the fixture as the run left it, so a later world sees the same
        // project the report describes.
        self.engine.redo()?;
        Ok(())
    }

    /// The `effect` world: describe it, then render a frame with it.
    fn check_effect(&self, plugin: &PluginId, component: &Component, report: &mut TestReport) {
        let world = World::Effect;
        let mut linker = match self.linker() {
            Ok(linker) => linker,
            Err(error) => return report.push(Check::failed_with(world, "links", &error)),
        };
        if let Err(error) =
            Effect::add_to_linker::<_, HasSelf<HarnessHost>>(&mut linker, |state| state)
                .map_err(|err| link_failed(&err))
        {
            return report.push(Check::failed_with(world, "links", &error));
        }
        let mut store = match self.store() {
            Ok(store) => store,
            Err(error) => return report.push(Check::failed_with(world, "instantiates", &error)),
        };
        let instance = match Effect::instantiate(&mut store, component, &linker) {
            Ok(instance) => instance,
            Err(err) => {
                let error = instantiate_failed(plugin, &err);
                return report.push(Check::failed_with(world, "instantiates", &error));
            }
        };
        report.push(Check::pass(
            world,
            "instantiates",
            "the component instantiated with every import satisfied",
        ));

        let desc = match instance.call_describe(&mut store) {
            Ok(desc) => desc,
            Err(err) => {
                let error = crate::errors::explain(termination(plugin, &err));
                return report.push(Check::failed_with(world, "describe", &error));
            }
        };
        report.log.extend(store.data().log().iter().cloned());
        report.push(
            Check::pass(world, "describe", "describe returned a declaration")
                .with("params", desc.params.len())
                .with("entry", desc.entry.clone())
                .with("shader_bytes", desc.shader.len()),
        );

        Self::render_frame(&desc, report);
    }

    /// Renders one frame with a plugin's declaration bound at its defaults.
    ///
    /// Only the `render` feature can do this: a host built without it — the
    /// MCP server, which never opens a device — says so rather than passing a
    /// check it did not run.
    #[cfg(feature = "render")]
    fn render_frame(desc: &crate::WitEffectDesc, report: &mut TestReport) {
        let world = World::Effect;
        let lifted = match crate::effect::effect_desc(desc) {
            Ok(lifted) => lifted,
            Err(error) => {
                return report.push(Check::failed_with(world, "declaration", &error));
            }
        };
        report.push(
            Check::pass(
                world,
                "declaration",
                "the declaration binds to the compositor's own types",
            )
            .with("uniform_bytes", lifted.layout().size()),
        );

        let context = match sub_render::RenderContext::headless() {
            Ok(context) => context,
            Err(sub_render::RenderError::NoAdapter { backends }) => {
                return report.push(Check::skip(
                    world,
                    "renders_frame",
                    format!("this machine offers no wgpu adapter for backends [{backends}]"),
                ));
            }
            Err(error) => {
                return report.push(
                    Check::fail(
                        world,
                        "renders_frame",
                        format!("no render device: [{}] {error}", error.code()),
                    )
                    .with("code", error.code()),
                );
            }
        };

        let probe = sub_render::probe_effect(&context, &lifted);
        if probe.applied {
            report.push(
                Check::pass(
                    world,
                    "renders_frame",
                    "the shader compiled and ran on a frame",
                )
                .with("input", pixel(sub_render::PROBE_INPUT))
                .with("output", pixel(probe.output))
                .with("changed_the_picture", probe.changed_the_picture())
                .with("adapter", context.describe()),
            );
        } else {
            let message = probe
                .message
                .clone()
                .unwrap_or_else(|| "the effect did not run on the frame".to_owned());
            report.push(
                Check::fail(world, "renders_frame", message)
                    .with("code", probe.code.unwrap_or("render.effect_not_applied"))
                    .with("adapter", context.describe()),
            );
        }
    }

    /// Without the `render` feature there is no compositor to render into.
    #[cfg(not(feature = "render"))]
    fn render_frame(_desc: &crate::WitEffectDesc, report: &mut TestReport) {
        report.push(Check::skip(
            World::Effect,
            "renders_frame",
            "this build of the harness has no compositor; rebuild it with the render feature",
        ));
    }

    /// The `analyzer` world: analyse the fixture's media and lift the
    /// findings.
    fn check_analyzer(&self, plugin: &PluginId, component: &Component, report: &mut TestReport) {
        let world = World::Analyzer;
        let mut linker = match self.linker() {
            Ok(linker) => linker,
            Err(error) => return report.push(Check::failed_with(world, "links", &error)),
        };
        if let Err(error) =
            AnalyzerWorld::add_to_linker::<_, HasSelf<HarnessHost>>(&mut linker, |state| state)
                .map_err(|err| link_failed(&err))
        {
            return report.push(Check::failed_with(world, "links", &error));
        }
        let mut store = match self.store() {
            Ok(store) => store,
            Err(error) => return report.push(Check::failed_with(world, "instantiates", &error)),
        };
        let instance = match AnalyzerWorld::instantiate(&mut store, component, &linker) {
            Ok(instance) => instance,
            Err(err) => {
                let error = instantiate_failed(plugin, &err);
                return report.push(Check::failed_with(world, "instantiates", &error));
            }
        };
        report.push(Check::pass(
            world,
            "instantiates",
            "the component instantiated with every import satisfied",
        ));

        // The fixture's own media where it has some, and a fresh identifier
        // where it has none: an analyzer is handed an id, never a path.
        let snapshot = self.engine.snapshot();
        let media = snapshot
            .media
            .first()
            .map_or_else(sub_model::MediaId::new, |item| item.id);
        let found = match instance.call_analyze(&mut store, &media.into(), &self.args) {
            Ok(Ok(found)) => found,
            Ok(Err(err)) => {
                let error = crate::errors::explain(SubError::from(err));
                report.log.extend(store.data().log().iter().cloned());
                return report.push(Check::failed_with(world, "analyze", &error));
            }
            Err(err) => {
                let error = crate::errors::explain(termination(plugin, &err));
                report.log.extend(store.data().log().iter().cloned());
                return report.push(Check::failed_with(world, "analyze", &error));
            }
        };
        report.log.extend(store.data().log().iter().cloned());
        let progress = store.data().progress().len();
        report.push(
            Check::pass(world, "analyze", "analyze returned findings")
                .with("media", media.to_string())
                .with("markers", found.markers.len())
                .with("ranges", found.ranges.len())
                .with("metadata", found.metadata.len())
                .with("progress_reports", progress),
        );

        match analysis_from_wit(plugin.as_str(), &found) {
            Ok(analysis) => report.push(
                Check::pass(
                    world,
                    "findings",
                    "every finding is one the project model can hold",
                )
                .with("markers", analysis.markers.len())
                .with("ranges", analysis.ranges.len()),
            ),
            Err(error) => report.push(Check::failed_with(world, "findings", &error)),
        }
    }

    /// The `mcp-tools` world: the exported tools must be the declared ones,
    /// and each must answer a call its own schema accepts.
    fn check_mcp_tools(
        &self,
        plugin: &PluginId,
        manifest: &Manifest,
        directory: &Path,
        component: &Component,
        report: &mut TestReport,
    ) {
        let world = World::McpTools;
        let catalog = match ToolCatalog::from_manifest(manifest, directory) {
            Ok(catalog) => catalog,
            Err(error) => return report.push(Check::failed_with(world, "declared_tools", &error)),
        };
        report.push(
            Check::pass(
                world,
                "declared_tools",
                "every tool the manifest declares has a schema this host can compile",
            )
            .with("tools", catalog.len()),
        );

        let mut linker = match self.linker() {
            Ok(linker) => linker,
            Err(error) => return report.push(Check::failed_with(world, "links", &error)),
        };
        if let Err(error) =
            McpTools::add_to_linker::<_, HasSelf<HarnessHost>>(&mut linker, |state| state)
                .map_err(|err| link_failed(&err))
        {
            return report.push(Check::failed_with(world, "links", &error));
        }
        let mut store = match self.store() {
            Ok(store) => store,
            Err(error) => return report.push(Check::failed_with(world, "instantiates", &error)),
        };
        let instance = match McpTools::instantiate(&mut store, component, &linker) {
            Ok(instance) => instance,
            Err(err) => {
                let error = instantiate_failed(plugin, &err);
                return report.push(Check::failed_with(world, "instantiates", &error));
            }
        };
        report.push(Check::pass(
            world,
            "instantiates",
            "the component instantiated with every import satisfied",
        ));

        let exported = match instance.call_tools(&mut store) {
            Ok(exported) => exported,
            Err(err) => {
                let error = crate::errors::explain(termination(plugin, &err));
                return report.push(Check::failed_with(world, "tools", &error));
            }
        };
        match catalog.accept_exports(&exported) {
            Ok(()) => report.push(
                Check::pass(
                    world,
                    "tools",
                    "the exported tools are exactly the ones the manifest declares",
                )
                .with(
                    "names",
                    exported
                        .iter()
                        .map(|tool| tool.name.clone())
                        .collect::<Vec<_>>(),
                ),
            ),
            Err(error) => {
                report.push(Check::failed_with(world, "tools", &error));
                return;
            }
        }

        self.call_tools(plugin, &catalog, &exported, &instance, &mut store, report);
        report.log.extend(store.data().log().iter().cloned());
    }

    /// Calls every exported tool with the run's arguments, once each.
    ///
    /// A tool whose own schema rejects those arguments is skipped rather than
    /// failed: the harness has no way to invent a document that satisfies an
    /// arbitrary schema, and saying so is more use than guessing.
    fn call_tools(
        &self,
        plugin: &PluginId,
        catalog: &ToolCatalog,
        exported: &[crate::WitToolDesc],
        instance: &McpTools,
        store: &mut wasmtime::Store<HarnessHost>,
        report: &mut TestReport,
    ) {
        let world = World::McpTools;
        for tool in exported {
            let name = format!("call:{}", tool.name);
            if let Err(error) = catalog.validate_arguments(&tool.name, &self.args) {
                report.push(
                    Check::skip(
                        world,
                        &name,
                        format!(
                            "the tool's schema rejects {}, so the harness has no arguments to \
                             call it with: {}",
                            self.args, error.message,
                        ),
                    )
                    .with_error(&error),
                );
                continue;
            }
            match instance.call_call(&mut *store, &tool.name, &self.args) {
                Ok(Ok(answer)) => report.push(
                    Check::pass(world, &name, "the tool answered")
                        .with("answer_bytes", answer.len())
                        .with(
                            "answer",
                            serde_json::from_str::<Value>(&answer).unwrap_or(Value::String(answer)),
                        ),
                ),
                Ok(Err(err)) => {
                    let error = crate::errors::explain(SubError::from(err));
                    report.push(Check::failed_with(world, &name, &error));
                }
                Err(err) => {
                    let error = crate::errors::explain(termination(plugin, &err));
                    report.push(Check::failed_with(world, &name, &error));
                }
            }
        }
    }

    /// The fixture's state as the report describes it: the revision plus the
    /// whole project as JSON, which is what makes "did undo put it back"
    /// answerable exactly rather than by counting.
    fn state(&self) -> SubResult<ProjectState> {
        let snapshot = self.engine.snapshot();
        Ok(ProjectState {
            revision: self.engine.revision(),
            sequences: snapshot.sequences.len(),
            tracks: snapshot
                .sequences
                .iter()
                .map(|sequence| sequence.tracks.len())
                .sum(),
            clips: snapshot
                .sequences
                .iter()
                .flat_map(|sequence| sequence.tracks.iter())
                .map(|track| track.clips().count())
                .sum(),
            markers: snapshot
                .sequences
                .iter()
                .map(|sequence| sequence.markers.len())
                .sum(),
            media: snapshot.media.len(),
            json: sub_model::json::to_json(&snapshot)?,
        })
    }
}

/// What the fixture looked like at one moment.
///
/// The revision is left out of the comparison on purpose: undo moves it on
/// like any other command, so what proves an edit undid cleanly is the
/// project itself.
#[derive(Debug, Clone)]
struct ProjectState {
    revision: u64,
    sequences: usize,
    tracks: usize,
    clips: usize,
    markers: usize,
    media: usize,
    json: String,
}

impl PartialEq for ProjectState {
    fn eq(&self, other: &Self) -> bool {
        self.json == other.json
    }
}

/// One state as the report prints it: the counts a person reads, without the
/// whole project JSON behind them.
fn state_json(state: &ProjectState) -> Value {
    json!({
        "revision": state.revision,
        "sequences": state.sequences,
        "tracks": state.tracks,
        "clips": state.clips,
        "markers": state.markers,
        "media": state.media,
    })
}

/// A pixel as the report prints it.
#[cfg(feature = "render")]
fn pixel(rgba: [u8; 4]) -> Value {
    json!({ "r": rgba[0], "g": rgba[1], "b": rgba[2], "a": rgba[3] })
}

/// A linker that could not be built or filled.
fn link_failed(err: &wasmtime::Error) -> SubError {
    crate::errors::explain(SubError::wrap(
        codes::LINK_FAILED,
        "the plugin's imports could not be satisfied by the host",
        err.as_ref(),
    ))
}

/// A component that compiled and linked but would not instantiate.
fn instantiate_failed(plugin: &PluginId, err: &wasmtime::Error) -> SubError {
    crate::errors::explain(
        SubError::wrap(
            codes::INSTANTIATE_FAILED,
            "the plugin component could not be instantiated",
            err.as_ref(),
        )
        .with_detail("plugin", plugin.as_str()),
    )
}
