//! TASK-9 spike: the `subordinate:plugin@0.1.0` `command` world, hosted.
//!
//! This crate is throwaway; the deliverable is the findings doc and the WIT in
//! `wit/subordinate-plugin.wit`. It exists to prove three things before the
//! other worlds are designed:
//!
//! 1. the shape of a host import — a plugin edits a project only by calling
//!    back into the Command API, never by holding the model;
//! 2. that a plain `cargo build --target wasm32-wasip2` guest links against
//!    that import and runs under [`wasmtime`];
//! 3. that a plugin which refuses to return can be stopped, by fuel for a
//!    deterministic budget and by an epoch deadline for a wall-clock one.
//!
//! The project it edits is [`FixtureProject`], a stand-in for `sub-model`,
//! which does not exist yet. Timeline positions are [`RationalTime`] on both
//! sides of the boundary; no float ever crosses it.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sub_core::{ErrorCode, SubError, SubResult};
use sub_time::{Rational, RationalTime};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

mod bindings {
    //! Host-side bindings generated from `wit/subordinate-plugin.wit`.
    wasmtime::component::bindgen!({
        path: "../../wit",
        world: "command",
    });
}

use bindings::Command;
use bindings::subordinate::plugin::command_api;
use bindings::subordinate::plugin::types::{Detail, Error as WitError};

/// Error codes this spike raises. The real host (TASK-84) owns the `plugin.*`
/// domain; these are the subset the spike needed.
pub mod codes {
    use sub_core::ErrorCode;

    /// A component file could not be read or is not a valid component.
    pub const LOAD_FAILED: ErrorCode = ErrorCode::from_static("plugin.load_failed");
    /// A component's imports could not be satisfied by the host.
    pub const LINK_FAILED: ErrorCode = ErrorCode::from_static("plugin.link_failed");
    /// A plugin trapped, ran out of fuel or overran its epoch deadline.
    pub const TRAPPED: ErrorCode = ErrorCode::from_static("plugin.trapped");
    /// A plugin asked for a Command API method the host does not have.
    pub const UNKNOWN_METHOD: ErrorCode = ErrorCode::from_static("plugin.unknown_method");
    /// A plugin's `params` were not the JSON the method expects.
    pub const INVALID_PARAMS: ErrorCode = ErrorCode::from_static("plugin.invalid_params");
    /// A plugin named a project the host does not have open.
    pub const NO_SUCH_PROJECT: ErrorCode = ErrorCode::from_static("plugin.no_such_project");
}

/// A marker on the fixture project's timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    /// The marker's name, as the plugin gave it.
    pub name: String,
    /// Where it sits, exactly.
    pub at: RationalTime,
}

/// The tiny project a plugin edits in this spike, standing in for `sub-model`.
///
/// Every mutation goes through [`FixtureProject::apply`], which is the
/// stand-in for one undoable Command; the spike records the applied methods so
/// a test can assert that the edit really travelled through the Command API
/// rather than around it.
#[derive(Debug, Clone)]
pub struct FixtureProject {
    id: String,
    playhead: RationalTime,
    markers: Vec<Marker>,
    applied: Vec<String>,
}

impl FixtureProject {
    /// Creates a project with the given id and playhead position.
    #[must_use]
    pub fn new(id: impl Into<String>, playhead: RationalTime) -> Self {
        Self {
            id: id.into(),
            playhead,
            markers: Vec::new(),
            applied: Vec::new(),
        }
    }

    /// The project's id, as plugins address it.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The current playhead position.
    #[must_use]
    pub fn playhead(&self) -> RationalTime {
        self.playhead
    }

    /// The markers added so far, in insertion order.
    #[must_use]
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    /// The Command API methods applied so far, in order.
    #[must_use]
    pub fn applied(&self) -> &[String] {
        &self.applied
    }

    /// Applies one mutating Command API call.
    ///
    /// # Errors
    ///
    /// Returns [`codes::UNKNOWN_METHOD`] for a method the fixture does not
    /// implement and [`codes::INVALID_PARAMS`] when `params` do not match it.
    pub fn apply(&mut self, method: &str, params: &str) -> SubResult<serde_json::Value> {
        let params: serde_json::Value = serde_json::from_str(params).map_err(|err| {
            SubError::wrap(codes::INVALID_PARAMS, "params are not valid JSON", &err)
                .with_detail("method", method)
        })?;

        match method {
            "sequence.add_marker" => {
                let name = params
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| invalid_params(method, "`name` must be a string"))?
                    .to_owned();
                let at = rational_time_from_json(params.get("at"), method)?;
                self.markers.push(Marker { name, at });
                self.applied.push(method.to_owned());
                Ok(serde_json::json!({ "marker_index": self.markers.len() - 1 }))
            }
            other => Err(
                SubError::new(codes::UNKNOWN_METHOD, "no such Command API method")
                    .with_detail("method", other),
            ),
        }
    }

    /// Answers one read-only Command API call.
    ///
    /// # Errors
    ///
    /// Returns [`codes::UNKNOWN_METHOD`] for a method the fixture does not
    /// implement.
    pub fn query(&self, method: &str, _params: &str) -> SubResult<serde_json::Value> {
        match method {
            "sequence.markers" => Ok(serde_json::Value::Array(
                self.markers
                    .iter()
                    .map(|marker| {
                        serde_json::json!({
                            "name": marker.name,
                            "at": {
                                "value": marker.at.value(),
                                "rate_numerator": marker.at.rate().numerator(),
                                "rate_denominator": marker.at.rate().denominator(),
                            },
                        })
                    })
                    .collect(),
            )),
            other => Err(
                SubError::new(codes::UNKNOWN_METHOD, "no such Command API query")
                    .with_detail("method", other),
            ),
        }
    }
}

/// Builds an [`codes::INVALID_PARAMS`] error for `method`.
fn invalid_params(method: &str, message: &str) -> SubError {
    SubError::new(codes::INVALID_PARAMS, message).with_detail("method", method)
}

/// Reads an `at` object as a [`RationalTime`].
///
/// The shape mirrors the WIT `rational-time` record:
/// `{ "value": i64, "rate_numerator": u32, "rate_denominator": u32 }`.
fn rational_time_from_json(
    value: Option<&serde_json::Value>,
    method: &str,
) -> SubResult<RationalTime> {
    let object = value.ok_or_else(|| invalid_params(method, "`at` is required"))?;
    let ticks = object
        .get("value")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| invalid_params(method, "`at.value` must be an integer"))?;
    let numerator = u32_field(object, "rate_numerator", method)?;
    let denominator = u32_field(object, "rate_denominator", method)?;
    let rate = Rational::new(numerator, denominator)
        .ok_or_else(|| invalid_params(method, "`at` rate is not a valid rational rate"))?;
    Ok(RationalTime::new(ticks, rate))
}

/// Reads a positive `u32` field out of a JSON object.
fn u32_field(object: &serde_json::Value, field: &str, method: &str) -> SubResult<u32> {
    object
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| invalid_params(method, &format!("`at.{field}` must be a positive integer")))
}

/// The resource ceilings a plugin instance runs under.
///
/// The two time limits answer different questions. Fuel counts executed
/// instructions, so the same plugin on the same input always stops at the same
/// place — reproducible, and what a test or a headless render wants. An epoch
/// deadline is wall-clock, so it also catches a plugin blocked in a long host
/// call, at the cost of being machine-dependent. The real host (TASK-84) wants
/// both: fuel for a per-call budget, epochs as the backstop.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Instruction budget, or `None` to leave fuel metering off.
    pub fuel: Option<u64>,
    /// Wall-clock budget, or `None` to leave epoch interruption off.
    pub deadline: Option<Duration>,
    /// Hard ceiling on the instance's linear memory, in bytes.
    pub memory_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            fuel: Some(50_000_000),
            deadline: Some(Duration::from_secs(2)),
            memory_bytes: 64 * 1024 * 1024,
        }
    }
}

impl Limits {
    /// Limits with fuel metering only, at `fuel` instructions.
    #[must_use]
    pub fn fuel_only(fuel: u64) -> Self {
        Self {
            fuel: Some(fuel),
            deadline: None,
            ..Self::default()
        }
    }

    /// Limits with an epoch deadline only, at `deadline` of wall-clock time.
    #[must_use]
    pub fn deadline_only(deadline: Duration) -> Self {
        Self {
            fuel: None,
            deadline: Some(deadline),
            ..Self::default()
        }
    }
}

/// What happened to one plugin call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The plugin returned successfully with this JSON payload.
    Completed(String),
    /// The plugin returned an error of its own across the boundary.
    Rejected(SubError),
    /// The host stopped the plugin: a trap, exhausted fuel or an overrun
    /// epoch deadline.
    Terminated(SubError),
}

impl Outcome {
    /// True when the host had to stop the plugin.
    #[must_use]
    pub fn is_terminated(&self) -> bool {
        matches!(self, Self::Terminated(_))
    }
}

/// One line a plugin wrote through `command-api.log`: level name and message.
pub type LogLine = (String, String);

/// Everything one plugin call produced.
#[derive(Debug)]
pub struct RunReport {
    /// How the call ended.
    pub outcome: Outcome,
    /// The project as the call left it.
    pub project: FixtureProject,
    /// The lines the plugin logged, in order.
    pub log: Vec<LogLine>,
    /// Instructions the call consumed, as wasmtime counts fuel.
    pub fuel_used: u64,
    /// Time spent instantiating the component, before `run` was entered.
    pub instantiation: Duration,
    /// Total wall-clock time of instantiation plus the call.
    pub elapsed: Duration,
}

/// The state one plugin instance sees.
struct HostState {
    project: FixtureProject,
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    log: Vec<LogLine>,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl bindings::subordinate::plugin::types::Host for HostState {}

impl command_api::Host for HostState {
    fn invoke(
        &mut self,
        project: String,
        method: String,
        params: String,
    ) -> Result<String, WitError> {
        self.check_project(&project)?;
        self.project
            .apply(&method, &params)
            .map(|value| value.to_string())
            .map_err(wit_error)
    }

    fn query(
        &mut self,
        project: String,
        method: String,
        params: String,
    ) -> Result<String, WitError> {
        self.check_project(&project)?;
        self.project
            .query(&method, &params)
            .map(|value| value.to_string())
            .map_err(wit_error)
    }

    fn playhead(&mut self, project: String) -> Result<command_api::RationalTime, WitError> {
        self.check_project(&project)?;
        let playhead = self.project.playhead();
        Ok(command_api::RationalTime {
            value: playhead.value(),
            rate_numerator: playhead.rate().numerator(),
            rate_denominator: playhead.rate().denominator(),
        })
    }

    fn log(&mut self, level: command_api::LogLevel, message: String) {
        let name = match level {
            command_api::LogLevel::Trace => {
                tracing::trace!(target: "plugin", "{message}");
                "trace"
            }
            command_api::LogLevel::Debug => {
                tracing::debug!(target: "plugin", "{message}");
                "debug"
            }
            command_api::LogLevel::Info => {
                tracing::info!(target: "plugin", "{message}");
                "info"
            }
            command_api::LogLevel::Warn => {
                tracing::warn!(target: "plugin", "{message}");
                "warn"
            }
            command_api::LogLevel::Error => {
                tracing::error!(target: "plugin", "{message}");
                "error"
            }
        };
        self.log.push((name.to_owned(), message));
    }
}

impl HostState {
    /// Rejects a call naming a project this host does not have open.
    fn check_project(&self, project: &str) -> Result<(), WitError> {
        if project == self.project.id() {
            return Ok(());
        }
        Err(wit_error(
            SubError::new(codes::NO_SUCH_PROJECT, "no such project")
                .with_detail("project", project),
        ))
    }
}

/// Converts a [`SubError`] into the WIT error record.
fn wit_error(err: SubError) -> WitError {
    WitError {
        code: err.code.to_string(),
        message: err.message,
        details: err
            .details
            .into_iter()
            .map(|(key, value)| Detail {
                key,
                value: value.to_string(),
            })
            .collect(),
    }
}

/// Converts the WIT error record back into a [`SubError`].
fn sub_error(err: &WitError) -> SubError {
    let mut details = BTreeMap::new();
    for detail in &err.details {
        let value = serde_json::from_str(&detail.value)
            .unwrap_or_else(|_| serde_json::Value::String(detail.value.clone()));
        details.insert(detail.key.clone(), value);
    }
    SubError {
        code: ErrorCode::parse(err.code.clone())
            .unwrap_or_else(|_| ErrorCode::from_static("plugin.invalid_error_code")),
        message: err.message.clone(),
        details,
        cause: None,
    }
}

/// A wasmtime engine plus the epoch ticker its deadlines need.
///
/// Cheap to clone in the sense that the engine is shared; one runtime should
/// serve every plugin instance in a process, because compiling a `Component`
/// is the expensive part and code is cached per engine.
pub struct PluginHost {
    engine: Engine,
    linker: Arc<Linker<HostState>>,
}

impl PluginHost {
    /// Builds a host with fuel metering and epoch interruption compiled in.
    ///
    /// # Errors
    ///
    /// Returns [`codes::LINK_FAILED`] if the engine or the WASI/host-import
    /// linker cannot be built.
    pub fn new() -> SubResult<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(|err| {
            SubError::wrap(
                codes::LINK_FAILED,
                "could not build a wasm engine",
                err.as_ref(),
            )
        })?;

        let mut linker = Linker::<HostState>::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|err| {
            SubError::wrap(codes::LINK_FAILED, "could not link WASI p2", err.as_ref())
        })?;
        Command::add_to_linker::<_, wasmtime::component::HasSelf<HostState>>(
            &mut linker,
            |state| state,
        )
        .map_err(|err| {
            SubError::wrap(
                codes::LINK_FAILED,
                "could not link the command world",
                err.as_ref(),
            )
        })?;

        Ok(Self {
            engine,
            linker: Arc::new(linker),
        })
    }

    /// Compiles a component from a `.wasm` file on disk.
    ///
    /// # Errors
    ///
    /// Returns [`codes::LOAD_FAILED`] when the file cannot be read or is not a
    /// valid component.
    pub fn load(&self, path: &Path) -> SubResult<Component> {
        Component::from_file(&self.engine, path).map_err(|err| {
            SubError::wrap(
                codes::LOAD_FAILED,
                "could not load plugin component",
                err.as_ref(),
            )
            .with_detail("path", path.display().to_string())
        })
    }

    /// Runs `component`'s `run` export against `project`.
    ///
    /// The report carries the (possibly mutated) project alongside the
    /// [`Outcome`], so a caller can see what a terminated plugin managed to
    /// change before it was stopped, plus what the call cost.
    ///
    /// # Errors
    ///
    /// Returns [`codes::LINK_FAILED`] when the component's imports cannot be
    /// satisfied. A plugin that traps, exhausts its fuel or overruns its
    /// deadline is not an error here: it is [`Outcome::Terminated`].
    pub fn run(
        &self,
        component: &Component,
        project: FixtureProject,
        args: &str,
        limits: Limits,
    ) -> SubResult<RunReport> {
        // One component becomes several core instances once its adapters are
        // linked in, so an `instances(1)` ceiling rejects every real plugin.
        let store_limits = StoreLimitsBuilder::new()
            .memory_size(limits.memory_bytes)
            .instances(64)
            .build();

        let mut store = Store::new(
            &self.engine,
            HostState {
                project,
                wasi: WasiCtxBuilder::new().build(),
                table: ResourceTable::new(),
                limits: store_limits,
                log: Vec::new(),
            },
        );
        store.limiter(|state| &mut state.limits);

        // Fuel is always metered (the engine has `consume_fuel`), so an
        // unlimited call still needs a budget set; use the largest there is.
        store
            .set_fuel(limits.fuel.unwrap_or(u64::MAX))
            .map_err(|err| {
                SubError::wrap(codes::LINK_FAILED, "could not set fuel", err.as_ref())
            })?;

        // With epoch interruption compiled in, a store that never has its
        // deadline set would trap on the first check. Push it out of reach when
        // no wall-clock limit was asked for.
        // No `epoch_deadline_callback`: on a sync store the default behaviour —
        // trap with `Trap::Interrupt` — is exactly what a runaway plugin should
        // get, and `UpdateDeadline::Yield` needs an async store.
        let ticker = limits.deadline.map(|deadline| {
            store.set_epoch_deadline(1);
            EpochTicker::start(self.engine.clone(), deadline)
        });
        if ticker.is_none() {
            store.set_epoch_deadline(u64::MAX);
        }

        let fuel_before = store.get_fuel().unwrap_or(0);
        let started = std::time::Instant::now();

        let instance =
            Command::instantiate(&mut store, component, &self.linker).map_err(|err| {
                SubError::wrap(
                    codes::LINK_FAILED,
                    "could not instantiate plugin",
                    err.as_ref(),
                )
            })?;
        let instantiated = started.elapsed();

        let project_id = store.data().project.id().to_owned();
        let called = instance.call_run(&mut store, &project_id, args);
        drop(ticker);

        let elapsed = started.elapsed();
        let fuel_after = store.get_fuel().unwrap_or(0);
        let outcome = match called {
            Ok(Ok(value)) => Outcome::Completed(value),
            Ok(Err(err)) => Outcome::Rejected(sub_error(&err)),
            Err(err) => Outcome::Terminated(termination_error(&err, Some(fuel_after))),
        };

        let state = store.into_data();
        Ok(RunReport {
            outcome,
            project: state.project,
            log: state.log,
            fuel_used: fuel_before.saturating_sub(fuel_after),
            instantiation: instantiated,
            elapsed,
        })
    }
}

/// Classifies why a plugin call ended abruptly.
///
/// The two limits are told apart by the `Trap` wasmtime attaches, not by
/// matching on the rendered message: `OutOfFuel` and `Interrupt` are stable
/// where the wording is not.
fn termination_error(err: &wasmtime::Error, fuel_left: Option<u64>) -> SubError {
    let rendered = format!("{err:?}");
    let reason = match err.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::OutOfFuel) => "plugin exhausted its instruction budget",
        Some(wasmtime::Trap::Interrupt) => "plugin overran its wall-clock deadline",
        _ => "plugin trapped",
    };
    let mut error = SubError::new(codes::TRAPPED, reason).with_detail("trap", rendered);
    if let Some(fuel) = fuel_left {
        error = error.with_detail("fuel_remaining", fuel);
    }
    error
}

/// A thread that increments the engine's epoch once `deadline` has passed, then
/// keeps incrementing until it is dropped.
///
/// wasmtime's own advice is one ticker per process; the spike starts one per
/// call because it needs each call's deadline measured from that call.
struct EpochTicker {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl EpochTicker {
    /// Starts a ticker that fires `deadline` from now.
    fn start(engine: Engine, deadline: Duration) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
                if started.elapsed() >= deadline {
                    engine.increment_epoch();
                }
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Paths of the guest components the build script produced.
///
/// `None` when the `wasm32-wasip2` target was not installed at build time.
pub mod guests {
    /// The plugin that adds a marker through the Command API host import.
    #[must_use]
    pub fn marker() -> Option<&'static std::path::Path> {
        path(option_env!("SPIKE_GUEST_MARKER"))
    }

    /// The plugin that loops forever.
    #[must_use]
    pub fn looper() -> Option<&'static std::path::Path> {
        path(option_env!("SPIKE_GUEST_LOOPER"))
    }

    /// Wraps a build-time path, if there was one.
    fn path(value: Option<&'static str>) -> Option<&'static std::path::Path> {
        value.map(std::path::Path::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> FixtureProject {
        FixtureProject::new("fixture", RationalTime::new(48, Rational::FPS_24))
    }

    #[test]
    fn fixture_add_marker_records_an_exact_time() {
        let mut project = fixture();
        let response = project
            .apply(
                "sequence.add_marker",
                r#"{"name":"beat","at":{"value":48,"rate_numerator":24,"rate_denominator":1}}"#,
            )
            .expect("add_marker should succeed");
        assert_eq!(response["marker_index"], 0);
        assert_eq!(project.markers()[0].name, "beat");
        assert_eq!(project.markers()[0].at.value(), 48);
        assert_eq!(project.applied(), ["sequence.add_marker"]);
    }

    #[test]
    fn fixture_rejects_an_unknown_method() {
        let mut project = fixture();
        let err = project.apply("sequence.nope", "{}").unwrap_err();
        assert_eq!(err.code.as_str(), "plugin.unknown_method");
    }

    #[test]
    fn error_records_round_trip_through_wit() {
        let original = SubError::new(codes::INVALID_PARAMS, "bad params")
            .with_detail("method", "sequence.add_marker");
        let round_tripped = sub_error(&wit_error(original.clone()));
        assert_eq!(round_tripped, original);
    }

    #[cfg(not(no_wasm_guests))]
    #[test]
    fn marker_plugin_edits_the_project_through_the_host_import() {
        let host = PluginHost::new().expect("host should build");
        let component = host
            .load(guests::marker().expect("marker guest was built"))
            .expect("marker guest should load");

        let report = host
            .run(
                &component,
                fixture(),
                r#"{"label":"from the plugin"}"#,
                Limits::default(),
            )
            .expect("the call should reach the plugin");

        match &report.outcome {
            Outcome::Completed(response) => assert!(response.contains("marker_index")),
            other => panic!("expected completion, got {other:?}"),
        }
        let project = &report.project;
        assert_eq!(project.markers().len(), 1);
        assert_eq!(project.markers()[0].name, "from the plugin");
        // The playhead crossed as a rational time and came back unchanged.
        assert_eq!(project.markers()[0].at, project.playhead());
        assert_eq!(project.applied(), ["sequence.add_marker"]);
        // The plugin's own log lines reached the host.
        assert_eq!(report.log.len(), 2);
        assert_eq!(report.log[0].0, "info");
        // A whole command call costs far less than the default budget.
        assert!(
            report.fuel_used < 1_000_000,
            "one command call burned {} fuel",
            report.fuel_used
        );
        eprintln!(
            "marker plugin: {} fuel, {:?} to instantiate, {:?} in total",
            report.fuel_used, report.instantiation, report.elapsed
        );
    }

    #[cfg(not(no_wasm_guests))]
    #[test]
    fn marker_plugin_error_crosses_back_as_a_sub_error() {
        let host = PluginHost::new().expect("host should build");
        let component = host
            .load(guests::marker().expect("marker guest was built"))
            .expect("marker guest should load");

        let report = host
            .run(&component, fixture(), "{}", Limits::default())
            .expect("the call should reach the plugin");

        match &report.outcome {
            Outcome::Rejected(err) => assert_eq!(err.code.as_str(), "plugin.invalid_argument"),
            other => panic!("expected a plugin error, got {other:?}"),
        }
        assert!(report.project.markers().is_empty());
    }

    #[cfg(not(no_wasm_guests))]
    #[test]
    fn fuel_exhaustion_terminates_a_looping_plugin() {
        let host = PluginHost::new().expect("host should build");
        let component = host
            .load(guests::looper().expect("looper guest was built"))
            .expect("looper guest should load");

        let report = host
            .run(&component, fixture(), "{}", Limits::fuel_only(1_000_000))
            .expect("the call should reach the plugin");

        let Outcome::Terminated(err) = &report.outcome else {
            panic!("expected termination, got {:?}", report.outcome);
        };
        assert_eq!(err.code.as_str(), "plugin.trapped");
        assert_eq!(err.message, "plugin exhausted its instruction budget");
        assert_eq!(report.fuel_used, 1_000_000);
    }

    #[cfg(not(no_wasm_guests))]
    #[test]
    fn an_epoch_deadline_terminates_a_looping_plugin() {
        let host = PluginHost::new().expect("host should build");
        let component = host
            .load(guests::looper().expect("looper guest was built"))
            .expect("looper guest should load");

        let deadline = Duration::from_millis(200);
        let report = host
            .run(&component, fixture(), "{}", Limits::deadline_only(deadline))
            .expect("the call should reach the plugin");

        let Outcome::Terminated(err) = &report.outcome else {
            panic!("expected termination, got {:?}", report.outcome);
        };
        assert_eq!(err.code.as_str(), "plugin.trapped");
        assert_eq!(err.message, "plugin overran its wall-clock deadline");
        // The ticker polls, so the stop is "soon after" the deadline, never
        // exactly on it. A wide bound keeps the test honest on a loaded box.
        assert!(
            report.elapsed >= deadline && report.elapsed < Duration::from_secs(30),
            "expected a stop shortly after {deadline:?}, got {:?}",
            report.elapsed
        );
        eprintln!(
            "epoch deadline {deadline:?} stopped the loop at {:?}",
            report.elapsed
        );
    }
}
