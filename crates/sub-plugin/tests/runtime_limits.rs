//! Real WASM components run under the host's limits (TASK-84).
//!
//! The guests in `tests/guests` are built for `wasm32-wasip2` by the build
//! script and run here through [`sub_plugin::PluginRuntime`]:
//!
//! - `looper` never returns, so it proves that fuel and an epoch deadline each
//!   stop a runaway plugin, that the two are told apart by stable error codes,
//!   and that stopping one leaves the engine and every other plugin working;
//! - `hog` allocates without bound, so it proves the memory ceiling is the
//!   thing that stops it rather than the host process running out;
//! - `counter` counts its calls in instance state, so a warm instance out of
//!   the [`sub_plugin::InstancePool`] is observably the same instance, with a
//!   full fuel budget again.
//!
//! When the `wasm32-wasip2` target is not installed the build script emits
//! `no_wasm_guests` and this file compiles down to nothing.

#![cfg(not(no_wasm_guests))]

use std::time::{Duration, Instant};

use sub_core::{ErrorCode, SubError, SubResult};
use sub_plugin::command_api::{
    ClipMetadata, Host, LogLevel, MarkerMetadata, ProjectMetadata, SequenceMetadata, TrackMetadata,
};
use sub_plugin::{
    Command, InstancePool, Limits, PluginId, PluginRuntime, PluginState, WarmInstance, WitError,
    WitProjectId, termination,
};
use wasmtime::StoreLimits;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// The project every guest here is pointed at. None of these guests edits it;
/// the Command API host functions exist so the world links.
const PROJECT: &str = "p-1";

/// The state one plugin instance sees.
struct HostState {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    log: Vec<String>,
}

impl HostState {
    fn new() -> Self {
        Self {
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
            limits: StoreLimits::default(),
            log: Vec::new(),
        }
    }
}

impl PluginState for HostState {
    fn store_limits(&mut self) -> &mut StoreLimits {
        &mut self.limits
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl sub_plugin::types::Host for HostState {}

/// The whole Command API, answered out of nothing: these guests never call it,
/// but the world does not link unless every import is satisfied.
impl Host for HostState {
    fn run_command(
        &mut self,
        _project: WitProjectId,
        method: String,
        _params: String,
    ) -> Result<String, WitError> {
        Err(unsupported(&method))
    }

    fn query(
        &mut self,
        _project: WitProjectId,
        method: String,
        _params: String,
    ) -> Result<String, WitError> {
        Err(unsupported(&method))
    }

    fn open_projects(&mut self) -> Vec<WitProjectId> {
        vec![WitProjectId {
            value: PROJECT.to_owned(),
        }]
    }

    fn project_info(&mut self, _project: WitProjectId) -> Result<ProjectMetadata, WitError> {
        Err(unsupported("project_info"))
    }

    fn sequences(&mut self, _project: WitProjectId) -> Result<Vec<SequenceMetadata>, WitError> {
        Err(unsupported("sequences"))
    }

    fn tracks(
        &mut self,
        _project: WitProjectId,
        _sequence: sub_plugin::WitSequenceId,
    ) -> Result<Vec<TrackMetadata>, WitError> {
        Err(unsupported("tracks"))
    }

    fn clips(
        &mut self,
        _project: WitProjectId,
        _sequence: sub_plugin::WitSequenceId,
        _track: sub_plugin::WitTrackId,
    ) -> Result<Vec<ClipMetadata>, WitError> {
        Err(unsupported("clips"))
    }

    fn markers(
        &mut self,
        _project: WitProjectId,
        _sequence: sub_plugin::WitSequenceId,
    ) -> Result<Vec<MarkerMetadata>, WitError> {
        Err(unsupported("markers"))
    }

    fn playhead(
        &mut self,
        _project: WitProjectId,
    ) -> Result<sub_plugin::WitRationalTime, WitError> {
        Err(unsupported("playhead"))
    }

    fn log(&mut self, _level: LogLevel, message: String) {
        self.log.push(message);
    }
}

/// The error every unimplemented accessor returns.
fn unsupported(what: &str) -> WitError {
    SubError::new(
        ErrorCode::from_static("plugin.unsupported"),
        "the fixture host answers no Command API calls",
    )
    .with_detail("method", what.to_owned())
    .into()
}

/// The guest components the build script produced.
mod guests {
    /// The plugin that never returns.
    pub fn looper() -> &'static std::path::Path {
        std::path::Path::new(env!("SUB_PLUGIN_GUEST_LOOPER"))
    }

    /// The plugin that allocates without bound.
    pub fn hog() -> &'static std::path::Path {
        std::path::Path::new(env!("SUB_PLUGIN_GUEST_HOG"))
    }

    /// The plugin that counts its calls in instance state.
    pub fn counter() -> &'static std::path::Path {
        std::path::Path::new(env!("SUB_PLUGIN_GUEST_COUNTER"))
    }
}

/// A runtime plus the one linker every world here shares.
struct Fixture {
    runtime: PluginRuntime,
    linker: Linker<HostState>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_tick(Duration::from_millis(5))
    }

    fn with_tick(tick: Duration) -> Self {
        let runtime = PluginRuntime::with_tick(tick).unwrap();
        let mut linker = Linker::<HostState>::new(runtime.engine());
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).unwrap();
        Command::add_to_linker::<_, HasSelf<HostState>>(&mut linker, |state| state).unwrap();
        Self { runtime, linker }
    }

    fn compile(&self, id: &PluginId, path: &std::path::Path) -> Component {
        self.runtime.compile_file(id, path).unwrap()
    }

    /// Instantiates `component` into a fresh store under `limits`.
    fn instantiate(
        &self,
        component: &Component,
        limits: Limits,
    ) -> SubResult<WarmInstance<HostState, Command>> {
        let mut store = self.runtime.store(HostState::new(), limits)?;
        let instance =
            Command::instantiate(&mut store, component, &self.linker).map_err(|err| {
                SubError::wrap(
                    ErrorCode::from_static("plugin.instantiate_failed"),
                    "could not instantiate plugin",
                    err.as_ref(),
                )
            })?;
        Ok(WarmInstance { store, instance })
    }

    /// Runs one call and reports either the guest's JSON or the host's
    /// termination error.
    fn run(
        id: &PluginId,
        warm: &mut WarmInstance<HostState, Command>,
        args: &str,
    ) -> Result<String, SubError> {
        let project = WitProjectId {
            value: PROJECT.to_owned(),
        };
        match warm.instance.call_run(&mut warm.store, &project, args) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(SubError::from(err)),
            Err(err) => Err(termination(id, &err)),
        }
    }
}

fn plugin(id: &str) -> PluginId {
    id.parse().unwrap()
}

#[test]
fn fuel_stops_a_plugin_that_never_returns() {
    let fixture = Fixture::new();
    let id = plugin("com.example.looper");
    let component = fixture.compile(&id, guests::looper());

    // Fuel only: the deadline is lifted so the stop is attributable to the
    // instruction budget alone, and is the same on every machine.
    let limits = Limits::default().with_fuel(1_000_000).without_deadline();
    let mut warm = fixture.instantiate(&component, limits).unwrap();

    let err = Fixture::run(&id, &mut warm, "{}").unwrap_err();
    assert_eq!(err.code.as_str(), "plugin.fuel_exhausted");
    assert_eq!(
        err.details.get("plugin").and_then(|v| v.as_str()),
        Some("com.example.looper")
    );
}

#[test]
fn an_epoch_deadline_stops_a_plugin_that_never_returns() {
    let fixture = Fixture::with_tick(Duration::from_millis(5));
    let id = plugin("com.example.looper");
    let component = fixture.compile(&id, guests::looper());

    // Deadline only: fuel is lifted, so nothing but the wall clock can stop
    // this call.
    let limits = Limits::default()
        .without_fuel()
        .with_deadline(Duration::from_millis(50));
    let mut warm = fixture.instantiate(&component, limits).unwrap();

    let started = Instant::now();
    let err = Fixture::run(&id, &mut warm, "{}").unwrap_err();
    let elapsed = started.elapsed();

    assert_eq!(err.code.as_str(), "plugin.deadline_exceeded");
    assert!(
        elapsed < Duration::from_secs(10),
        "the deadline should have fired long before this: {elapsed:?}"
    );
}

#[test]
fn stopping_one_plugin_leaves_the_others_running() {
    let fixture = Fixture::new();
    let looper_id = plugin("com.example.looper");
    let counter_id = plugin("com.example.counter");
    let looper = fixture.compile(&looper_id, guests::looper());
    let counter = fixture.compile(&counter_id, guests::counter());

    // The well-behaved plugin is instantiated first, so it is alive on the
    // same engine while the runaway one is stopped.
    let good_limits = Limits::default().with_fuel(50_000_000);
    let mut good = fixture.instantiate(&counter, good_limits).unwrap();
    assert_eq!(
        Fixture::run(&counter_id, &mut good, "{}").unwrap(),
        r#"{"calls":1}"#
    );

    let mut bad = fixture
        .instantiate(&looper, Limits::default().with_fuel(500_000))
        .unwrap();
    let err = Fixture::run(&looper_id, &mut bad, "{}").unwrap_err();
    assert_eq!(err.code.as_str(), "plugin.fuel_exhausted");
    drop(bad);

    // The engine, the compiled components and the other plugin's store are
    // untouched: the surviving instance keeps its state and keeps answering.
    assert_eq!(
        Fixture::run(&counter_id, &mut good, "{}").unwrap(),
        r#"{"calls":2}"#
    );

    // And a plugin instantiated after the termination works too.
    let mut fresh = fixture.instantiate(&counter, good_limits).unwrap();
    assert_eq!(
        Fixture::run(&counter_id, &mut fresh, "{}").unwrap(),
        r#"{"calls":1}"#
    );
}

#[test]
fn the_memory_ceiling_stops_a_plugin_that_allocates_without_bound() {
    let fixture = Fixture::new();
    let id = plugin("com.example.hog");
    let component = fixture.compile(&id, guests::hog());

    // Sixteen megabytes is far more than the guest needs to start and far less
    // than it wants; fuel is lifted so the ceiling is what stops it.
    let limits = Limits::default()
        .with_memory_bytes(16 * 1024 * 1024)
        .without_fuel()
        .with_deadline(Duration::from_secs(30));
    let mut warm = fixture.instantiate(&component, limits).unwrap();

    let err = Fixture::run(&id, &mut warm, "{}").unwrap_err();
    assert_eq!(
        err.code.as_str(),
        "plugin.trapped",
        "a refused allocation should surface as a trap, not a deadline: {err:?}"
    );
}

#[test]
fn a_pooled_instance_is_reused_and_refuelled() {
    let fixture = Fixture::new();
    let id = plugin("com.example.counter");
    let component = fixture.compile(&id, guests::counter());
    let limits = Limits::default().with_fuel(20_000_000);
    let mut pool: InstancePool<HostState, Command> =
        InstancePool::new(fixture.runtime.clone(), limits, 2);

    let make = |_: &PluginRuntime, limits: Limits| fixture.instantiate(&component, limits);

    // First call: a miss, so a fresh instance, which has seen one call.
    let mut warm = pool.checkout(&id, make).unwrap();
    assert_eq!(
        Fixture::run(&id, &mut warm, "{}").unwrap(),
        r#"{"calls":1}"#
    );
    let after_first = warm.store.get_fuel().unwrap();
    assert!(after_first < limits.fuel.unwrap(), "the call burned fuel");
    pool.release(&id, warm);

    // Second call: a hit. The guest's own counter says it is the same
    // instance, and the budget is whole again rather than what the first call
    // left behind.
    let mut warm = pool.checkout(&id, make).unwrap();
    assert_eq!(warm.store.get_fuel().unwrap(), limits.fuel.unwrap());
    assert_eq!(
        Fixture::run(&id, &mut warm, "{}").unwrap(),
        r#"{"calls":2}"#
    );
    pool.release(&id, warm);

    assert_eq!((pool.hits(), pool.misses()), (1, 1));

    // Eviction is what a hot reload does: the next checkout builds a fresh
    // instance, whose counter starts again.
    pool.evict(&id);
    let mut warm = pool.checkout(&id, make).unwrap();
    assert_eq!(
        Fixture::run(&id, &mut warm, "{}").unwrap(),
        r#"{"calls":1}"#
    );
    assert_eq!((pool.hits(), pool.misses()), (1, 2));
}

#[test]
fn a_terminated_instance_is_dropped_rather_than_pooled() {
    let fixture = Fixture::new();
    let id = plugin("com.example.looper");
    let component = fixture.compile(&id, guests::looper());
    let limits = Limits::default().with_fuel(500_000);
    let mut pool: InstancePool<HostState, Command> =
        InstancePool::new(fixture.runtime.clone(), limits, 2);

    let make = |_: &PluginRuntime, limits: Limits| fixture.instantiate(&component, limits);

    let mut warm = pool.checkout(&id, make).unwrap();
    let err = Fixture::run(&id, &mut warm, "{}").unwrap_err();
    assert_eq!(err.code.as_str(), "plugin.fuel_exhausted");
    // The caller drops a terminated instance instead of releasing it.
    drop(warm);

    assert_eq!(pool.warm_count(&id), 0);
    pool.checkout(&id, make).unwrap();
    assert_eq!(pool.misses(), 2);
}
