//! The wasmtime host: instances, resource limits and warm-instance pooling.
//!
//! A bad plugin must not stall or crash the engine (docs/PLAN.md §4), so every
//! plugin call runs inside limits the host sets before the guest gets control:
//!
//! - **Fuel** counts executed instructions. It is deterministic — the same
//!   plugin on the same input stops at the same place — which is what a test,
//!   a headless render or a reproducible export wants. It is the per-call CPU
//!   budget.
//! - **An epoch deadline** is wall-clock. It catches what fuel cannot: a guest
//!   parked in a long host call, or one whose instruction budget is generous
//!   but whose work is slow. One [`PluginRuntime`] runs a single ticker thread
//!   that bumps the engine's epoch every [`PluginRuntime::tick`], and each
//!   store's deadline is a number of those ticks, so a hundred plugin
//!   instances still cost one thread.
//! - **A memory ceiling**, applied through wasmtime's [`StoreLimits`], caps
//!   the instance's linear memory, its tables and how many core instances one
//!   component may create. A guest that asks for more sees `memory.grow` fail;
//!   the host process is never the one that runs out.
//!
//! [`Limits`] carries all three. [`PluginRuntime::store`] builds a store that
//! obeys them, and [`PluginRuntime::rearm`] resets fuel and the deadline
//! before a warm instance is used again.
//!
//! # Terminations are ordinary errors
//!
//! When a limit fires, wasmtime unwinds the guest and the call comes back as a
//! [`wasmtime::Error`]. [`termination`] turns it into a [`SubError`] with a
//! stable code — [`codes::FUEL_EXHAUSTED`], [`codes::DEADLINE_EXCEEDED`] or
//! [`codes::TRAPPED`] — telling the two limits apart by the [`wasmtime::Trap`]
//! wasmtime attaches rather than by matching on rendered text. The store the
//! call ran in is finished, but nothing else is: the engine, its compiled
//! components and every other plugin's store are untouched, so one runaway
//! plugin is reported and the rest keep working.
//!
//! # Pooling
//!
//! Instantiating a component is not free — the WASI adapter alone is several
//! core instances — and some calls are hot: an effect's `describe` after a
//! parameter edit, a plugin command run from a menu, a tool call from an
//! agent. [`InstancePool`] keeps a bounded number of warm instances per
//! plugin. [`InstancePool::checkout`] hands one back with its fuel and
//! deadline reset, or builds one on a miss; [`InstancePool::release`] returns
//! it. A call that ended in a termination must *not* be released — its store
//! is unwound and its guest state is arbitrary — so the caller simply drops
//! it and the next checkout builds a fresh one. [`InstancePool::evict`] drops
//! a plugin's warm instances outright, which is what an uninstall or a hot
//! reload (TASK-86) needs.
//!
//! # What this module does not decide
//!
//! It is generic over the store's data type: a caller keeps whatever state its
//! world needs — the [`WasiCtx`](wasmtime_wasi::WasiCtx) that
//! [`ResolvedCapabilities::apply_to_wasi`](crate::ResolvedCapabilities::apply_to_wasi)
//! built, the project handle its `Host` impl reads — and implements
//! [`PluginState`] so the runtime can reach the [`StoreLimits`] inside it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use sub_core::{SubError, SubResult};
use wasmtime::component::Component;
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap};

use crate::codes;
use crate::manifest::PluginId;

/// The default instruction budget for one plugin call.
///
/// Generous enough that ordinary work — parsing a file, walking a sequence,
/// building a parameter schema — never notices it, small enough that a runaway
/// loop stops in well under a second on any machine this ships to.
pub const DEFAULT_FUEL: u64 = 50_000_000;

/// The default wall-clock budget for one plugin call.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(2);

/// The default ceiling on one instance's linear memory.
pub const DEFAULT_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// The deadline a store with no wall-clock limit gets, in ticks.
///
/// Not `u64::MAX`: wasmtime adds the delta to the engine's current epoch, so a
/// deadline that large overflows. Half the range is out of reach either way —
/// at one tick a millisecond it is some hundred million years — while leaving
/// room for every epoch the process will ever count.
const NO_DEADLINE_TICKS: u64 = u64::MAX / 2;

/// How often the epoch ticker bumps the engine's epoch by default.
///
/// This is the resolution of every deadline: a limit of 250 ms is really "at
/// the first tick at or after 250 ms". Ten milliseconds keeps the thread
/// asleep almost always while still stopping a runaway plugin promptly.
pub const DEFAULT_TICK: Duration = Duration::from_millis(10);

/// The resource ceilings one plugin instance runs under.
///
/// See the module docs for why all three are needed. `fuel` and `deadline` may
/// each be `None`, which lifts that one limit; the memory ceiling is never
/// optional, because an unbounded guest allocation is the one failure that
/// takes the host process with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Instruction budget for one call, or `None` for no fuel limit.
    pub fuel: Option<u64>,
    /// Wall-clock budget for one call, or `None` for no deadline.
    pub deadline: Option<Duration>,
    /// Hard ceiling on the instance's linear memory, in bytes.
    pub memory_bytes: usize,
    /// Hard ceiling on the instance's table elements.
    pub table_elements: usize,
    /// How many core instances one component may create. A component is
    /// several core instances once its adapters are linked in, so this is
    /// never 1.
    pub instances: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            fuel: Some(DEFAULT_FUEL),
            deadline: Some(DEFAULT_DEADLINE),
            memory_bytes: DEFAULT_MEMORY_BYTES,
            table_elements: 100_000,
            instances: 64,
        }
    }
}

impl Limits {
    /// The defaults with the fuel budget replaced.
    #[must_use]
    pub fn with_fuel(mut self, fuel: u64) -> Self {
        self.fuel = Some(fuel);
        self
    }

    /// The defaults with the wall-clock deadline replaced.
    #[must_use]
    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// The defaults with the memory ceiling replaced.
    #[must_use]
    pub fn with_memory_bytes(mut self, bytes: usize) -> Self {
        self.memory_bytes = bytes;
        self
    }

    /// The same limits with fuel metering lifted.
    #[must_use]
    pub fn without_fuel(mut self) -> Self {
        self.fuel = None;
        self
    }

    /// The same limits with the wall-clock deadline lifted.
    #[must_use]
    pub fn without_deadline(mut self) -> Self {
        self.deadline = None;
        self
    }

    /// The [`StoreLimits`] these ceilings become.
    fn store_limits(self) -> StoreLimits {
        StoreLimitsBuilder::new()
            .memory_size(self.memory_bytes)
            .table_elements(self.table_elements)
            .instances(self.instances)
            .build()
    }

    /// The number of epoch ticks `deadline` is worth, at least one so a
    /// deadline shorter than a tick still fires at the next one.
    fn deadline_ticks(self, tick: Duration) -> Option<u64> {
        let deadline = self.deadline?;
        let tick = tick.max(Duration::from_nanos(1)).as_nanos();
        let ticks = deadline.as_nanos().div_ceil(tick);
        Some(u64::try_from(ticks).unwrap_or(u64::MAX).max(1))
    }
}

/// The state a plugin store carries.
///
/// The runtime needs exactly one thing out of it — the [`StoreLimits`] the
/// resource limiter reads — and leaves everything else (the WASI context, the
/// project handle, the log sink) to the caller's own type.
pub trait PluginState: 'static {
    /// The limits this store enforces. The runtime installs it as the store's
    /// resource limiter, so a guest's `memory.grow` is checked against it.
    fn store_limits(&mut self) -> &mut StoreLimits;
}

/// A shared wasmtime engine with fuel metering, epoch interruption and one
/// ticker thread.
///
/// Cloning is cheap and shares everything: compiled code is cached per engine,
/// so a process should have one runtime and hand clones around rather than
/// build a second engine per plugin.
#[derive(Clone)]
pub struct PluginRuntime {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for PluginRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRuntime")
            .field("tick", &self.inner.tick)
            .finish_non_exhaustive()
    }
}

struct Inner {
    engine: Engine,
    tick: Duration,
    /// Held only for its `Drop`: the ticker thread stops when the last clone
    /// of the runtime goes away.
    _ticker: Ticker,
}

impl PluginRuntime {
    /// Builds a runtime with the default epoch tick.
    ///
    /// # Errors
    ///
    /// Returns [`codes::ENGINE_FAILED`] if wasmtime rejects the configuration,
    /// which on a supported host means the build lacks a compiler.
    pub fn new() -> SubResult<Self> {
        Self::with_tick(DEFAULT_TICK)
    }

    /// Builds a runtime whose epoch ticker fires every `tick`.
    ///
    /// A shorter tick makes deadlines sharper and wakes the ticker thread more
    /// often; it is the resolution of every [`Limits::deadline`] in the
    /// process.
    ///
    /// # Errors
    ///
    /// Returns [`codes::ENGINE_FAILED`] if wasmtime rejects the configuration.
    pub fn with_tick(tick: Duration) -> SubResult<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(|err| {
            SubError::wrap(
                codes::ENGINE_FAILED,
                "could not build the plugin wasm engine",
                err.as_ref(),
            )
        })?;

        let tick = tick.max(Duration::from_millis(1));
        let ticker = Ticker::start(engine.clone(), tick);
        Ok(Self {
            inner: Arc::new(Inner {
                engine,
                tick,
                _ticker: ticker,
            }),
        })
    }

    /// The engine every plugin instance shares.
    #[must_use]
    pub fn engine(&self) -> &Engine {
        &self.inner.engine
    }

    /// How often the epoch ticker fires: the resolution of every deadline.
    #[must_use]
    pub fn tick(&self) -> Duration {
        self.inner.tick
    }

    /// Compiles a component from bytes.
    ///
    /// # Errors
    ///
    /// Returns [`codes::LOAD_FAILED`] when the bytes are not a component this
    /// engine can compile.
    pub fn compile(&self, plugin: &PluginId, bytes: &[u8]) -> SubResult<Component> {
        Component::new(&self.inner.engine, bytes).map_err(|err| {
            SubError::wrap(
                codes::LOAD_FAILED,
                "could not compile plugin component",
                err.as_ref(),
            )
            .with_detail("plugin", plugin.as_str())
        })
    }

    /// Compiles a component from a `.wasm` file on disk.
    ///
    /// # Errors
    ///
    /// Returns [`codes::LOAD_FAILED`] when the file cannot be read or is not a
    /// component this engine can compile.
    pub fn compile_file(&self, plugin: &PluginId, path: &Path) -> SubResult<Component> {
        Component::from_file(&self.inner.engine, path).map_err(|err| {
            SubError::wrap(
                codes::LOAD_FAILED,
                "could not load plugin component",
                err.as_ref(),
            )
            .with_detail("plugin", plugin.as_str())
            .with_detail("path", path.display().to_string())
        })
    }

    /// Builds a store for one plugin instance, limited by `limits`.
    ///
    /// The caller's `state` keeps its own [`StoreLimits`]; the runtime
    /// overwrites it with the one `limits` describes, installs it as the
    /// store's resource limiter and arms fuel and the epoch deadline, so the
    /// store is ready to instantiate a component into.
    ///
    /// # Errors
    ///
    /// Returns [`codes::ENGINE_FAILED`] if fuel cannot be set, which only
    /// happens when the engine was built without fuel metering.
    pub fn store<T: PluginState>(&self, mut state: T, limits: Limits) -> SubResult<Store<T>> {
        *state.store_limits() = limits.store_limits();
        let mut store = Store::new(&self.inner.engine, state);
        store.limiter(|state| state.store_limits());
        self.rearm(&mut store, limits)?;
        Ok(store)
    }

    /// Resets one store's fuel and epoch deadline to `limits`.
    ///
    /// This is what makes a warm instance safe to reuse: the previous call's
    /// spending does not carry over, so a pooled instance gets a whole budget
    /// per call exactly like a fresh one.
    ///
    /// # Errors
    ///
    /// Returns [`codes::ENGINE_FAILED`] if fuel cannot be set.
    pub fn rearm<T>(&self, store: &mut Store<T>, limits: Limits) -> SubResult<()> {
        // Fuel is always metered because the engine has `consume_fuel`, so a
        // call with no budget of its own still needs one set: the largest
        // there is.
        store
            .set_fuel(limits.fuel.unwrap_or(u64::MAX))
            .map_err(|err| {
                SubError::wrap(
                    codes::ENGINE_FAILED,
                    "could not set the plugin fuel budget",
                    err.as_ref(),
                )
            })?;

        // With epoch interruption compiled in, a store whose deadline was
        // never set traps at the first check, so an unlimited call gets a
        // deadline out of reach instead. No `epoch_deadline_callback`: on a
        // sync store the default — trap with `Trap::Interrupt` — is exactly
        // what a runaway plugin should get, and `UpdateDeadline::Yield` needs
        // an async store.
        match limits.deadline_ticks(self.inner.tick) {
            Some(ticks) => store.set_epoch_deadline(ticks),
            None => store.set_epoch_deadline(NO_DEADLINE_TICKS),
        }
        Ok(())
    }

    /// The fuel `store` has left, or zero when metering is off.
    #[must_use]
    pub fn fuel_remaining<T>(store: &Store<T>) -> u64 {
        store.get_fuel().unwrap_or(0)
    }
}

/// Why a plugin call ended abruptly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    /// The call ran out of instructions.
    Fuel,
    /// The call overran its wall-clock deadline.
    Deadline,
    /// The guest trapped: unreachable code, a failed allocation, a division by
    /// zero, an out-of-bounds access.
    Trap,
}

impl Termination {
    /// The stable error code this termination is reported under.
    #[must_use]
    pub fn code(self) -> sub_core::ErrorCode {
        match self {
            Self::Fuel => codes::FUEL_EXHAUSTED,
            Self::Deadline => codes::DEADLINE_EXCEEDED,
            Self::Trap => codes::TRAPPED,
        }
    }

    /// The message this termination is reported with.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::Fuel => "plugin exhausted its instruction budget",
            Self::Deadline => "plugin overran its wall-clock deadline",
            Self::Trap => "plugin trapped",
        }
    }

    /// Classifies one wasmtime error.
    ///
    /// The two limits are told apart by the [`Trap`] wasmtime attaches, not by
    /// matching on the rendered message: `OutOfFuel` and `Interrupt` are
    /// stable where the wording is not.
    #[must_use]
    pub fn classify(err: &wasmtime::Error) -> Self {
        match err.downcast_ref::<Trap>() {
            Some(Trap::OutOfFuel) => Self::Fuel,
            Some(Trap::Interrupt) => Self::Deadline,
            _ => Self::Trap,
        }
    }
}

/// Turns a wasmtime error from a plugin call into a [`SubError`] with a stable
/// code.
///
/// The `plugin` detail names which plugin was stopped and `trap` carries
/// wasmtime's own rendering, so a log line says what happened without the
/// caller having to keep the error around.
#[must_use]
pub fn termination(plugin: &PluginId, err: &wasmtime::Error) -> SubError {
    let reason = Termination::classify(err);
    SubError::new(reason.code(), reason.message())
        .with_detail("plugin", plugin.as_str())
        .with_detail("trap", format!("{err:?}"))
}

/// One warm plugin instance: the store it lives in and the world handle that
/// calls it.
///
/// `I` is whatever the world's bindings produced — [`crate::Command`],
/// [`crate::Effect`], [`crate::AudioEffect`] — so a pool is typed to one
/// world.
pub struct WarmInstance<T: 'static, I> {
    /// The store the instance lives in, holding the caller's state.
    pub store: Store<T>,
    /// The instantiated world.
    pub instance: I,
}

impl<T: 'static, I> std::fmt::Debug for WarmInstance<T, I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WarmInstance").finish_non_exhaustive()
    }
}

/// A bounded set of warm instances per plugin.
///
/// See the module docs: [`checkout`](Self::checkout) reuses a warm instance
/// with its budgets reset or builds one on a miss, [`release`](Self::release)
/// returns a healthy instance, and an instance whose call was terminated is
/// simply dropped instead of being released.
pub struct InstancePool<T: 'static, I> {
    runtime: PluginRuntime,
    limits: Limits,
    per_plugin: usize,
    warm: HashMap<PluginId, Vec<WarmInstance<T, I>>>,
    hits: u64,
    misses: u64,
}

impl<T: 'static, I> std::fmt::Debug for InstancePool<T, I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstancePool")
            .field("limits", &self.limits)
            .field("per_plugin", &self.per_plugin)
            .field("plugins", &self.warm.len())
            .field("hits", &self.hits)
            .field("misses", &self.misses)
            .finish_non_exhaustive()
    }
}

impl<T: PluginState, I> InstancePool<T, I> {
    /// A pool that keeps at most `per_plugin` warm instances of each plugin,
    /// each running under `limits`. A `per_plugin` of zero is read as one.
    #[must_use]
    pub fn new(runtime: PluginRuntime, limits: Limits, per_plugin: usize) -> Self {
        Self {
            runtime,
            limits,
            per_plugin: per_plugin.max(1),
            warm: HashMap::new(),
            hits: 0,
            misses: 0,
        }
    }

    /// The runtime every pooled instance shares.
    #[must_use]
    pub fn runtime(&self) -> &PluginRuntime {
        &self.runtime
    }

    /// The limits each checkout arms.
    #[must_use]
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Takes a warm instance of `plugin`, or builds one with `make`.
    ///
    /// A warm instance comes back with its fuel and epoch deadline reset, so
    /// the caller need not know whether it was hot or cold.
    ///
    /// # Errors
    ///
    /// Returns whatever `make` returns on a miss, and
    /// [`codes::ENGINE_FAILED`] if a warm store's budgets cannot be reset.
    pub fn checkout<F>(&mut self, plugin: &PluginId, make: F) -> SubResult<WarmInstance<T, I>>
    where
        F: FnOnce(&PluginRuntime, Limits) -> SubResult<WarmInstance<T, I>>,
    {
        if let Some(warm) = self.warm.get_mut(plugin).and_then(Vec::pop) {
            let mut warm = warm;
            self.runtime.rearm(&mut warm.store, self.limits)?;
            self.hits += 1;
            return Ok(warm);
        }
        self.misses += 1;
        make(&self.runtime, self.limits)
    }

    /// Returns a healthy instance to the pool.
    ///
    /// The instance is dropped instead if `plugin` is already at its warm
    /// ceiling. Never release an instance whose call was terminated: its store
    /// is unwound and the guest's own state is arbitrary from then on.
    pub fn release(&mut self, plugin: &PluginId, instance: WarmInstance<T, I>) {
        let warm = self.warm.entry(plugin.clone()).or_default();
        if warm.len() < self.per_plugin {
            warm.push(instance);
        }
    }

    /// Drops every warm instance of `plugin`, which an uninstall or a hot
    /// reload must do before the component behind them changes.
    pub fn evict(&mut self, plugin: &PluginId) {
        self.warm.remove(plugin);
    }

    /// Drops every warm instance of every plugin.
    pub fn clear(&mut self) {
        self.warm.clear();
    }

    /// How many warm instances of `plugin` the pool is holding.
    #[must_use]
    pub fn warm_count(&self, plugin: &PluginId) -> usize {
        self.warm.get(plugin).map_or(0, Vec::len)
    }

    /// Checkouts that reused a warm instance.
    #[must_use]
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Checkouts that had to build an instance.
    #[must_use]
    pub fn misses(&self) -> u64 {
        self.misses
    }
}

/// The thread that bumps the engine's epoch on a fixed period.
///
/// One per [`PluginRuntime`], stopped when the last clone of the runtime is
/// dropped. Deadlines are counted in these ticks, so a store with no deadline
/// costs nothing and a store with one costs no thread of its own.
struct Ticker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Ticker {
    fn start(engine: Engine, tick: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("sub-plugin-epoch".to_owned())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(tick);
                    engine.increment_epoch();
                }
            })
            .ok();
        Self { stop, handle }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_id() -> PluginId {
        "com.example.test".parse().unwrap()
    }

    #[test]
    fn a_deadline_shorter_than_a_tick_still_fires_at_the_next_tick() {
        let limits = Limits::default().with_deadline(Duration::from_millis(1));
        assert_eq!(limits.deadline_ticks(Duration::from_millis(10)), Some(1));
    }

    #[test]
    fn a_deadline_rounds_up_to_whole_ticks() {
        let limits = Limits::default().with_deadline(Duration::from_millis(25));
        assert_eq!(limits.deadline_ticks(Duration::from_millis(10)), Some(3));
    }

    #[test]
    fn lifting_the_deadline_leaves_no_ticks_to_count() {
        assert_eq!(
            Limits::default()
                .without_deadline()
                .deadline_ticks(DEFAULT_TICK),
            None
        );
    }

    #[test]
    fn a_runtime_shares_one_engine_across_clones() {
        let runtime = PluginRuntime::new().unwrap();
        let clone = runtime.clone();
        assert!(Engine::same(runtime.engine(), clone.engine()));
        assert_eq!(clone.tick(), runtime.tick());
    }

    #[test]
    fn a_tick_never_drops_below_a_millisecond() {
        let runtime = PluginRuntime::with_tick(Duration::ZERO).unwrap();
        assert_eq!(runtime.tick(), Duration::from_millis(1));
    }

    /// The pool is generic over the world handle, so a unit test can stand a
    /// plain state and a unit "instance" in for one.
    struct State {
        limits: StoreLimits,
    }

    impl PluginState for State {
        fn store_limits(&mut self) -> &mut StoreLimits {
            &mut self.limits
        }
    }

    fn warm(runtime: &PluginRuntime, limits: Limits) -> SubResult<WarmInstance<State, ()>> {
        Ok(WarmInstance {
            store: runtime.store(
                State {
                    limits: StoreLimits::default(),
                },
                limits,
            )?,
            instance: (),
        })
    }

    #[test]
    fn a_released_instance_is_the_next_checkout() {
        let runtime = PluginRuntime::new().unwrap();
        let mut pool = InstancePool::new(runtime, Limits::default(), 2);
        let id = plugin_id();

        let instance = pool.checkout(&id, warm).unwrap();
        assert_eq!((pool.hits(), pool.misses()), (0, 1));
        pool.release(&id, instance);
        assert_eq!(pool.warm_count(&id), 1);

        let instance = pool.checkout(&id, warm).unwrap();
        assert_eq!((pool.hits(), pool.misses()), (1, 1));
        assert_eq!(pool.warm_count(&id), 0);
        drop(instance);
    }

    #[test]
    fn a_checkout_refuels_the_warm_store() {
        let runtime = PluginRuntime::new().unwrap();
        let limits = Limits::default().with_fuel(1_000);
        let mut pool = InstancePool::new(runtime, limits, 1);
        let id = plugin_id();

        let mut instance = pool.checkout(&id, warm).unwrap();
        instance.store.set_fuel(7).unwrap();
        pool.release(&id, instance);

        let instance = pool.checkout(&id, warm).unwrap();
        assert_eq!(PluginRuntime::fuel_remaining(&instance.store), 1_000);
    }

    #[test]
    fn the_pool_holds_no_more_than_its_ceiling() {
        let runtime = PluginRuntime::new().unwrap();
        let mut pool = InstancePool::new(runtime, Limits::default(), 1);
        let id = plugin_id();

        let first = pool.checkout(&id, warm).unwrap();
        let second = pool.checkout(&id, warm).unwrap();
        pool.release(&id, first);
        pool.release(&id, second);
        assert_eq!(pool.warm_count(&id), 1);
    }

    #[test]
    fn eviction_drops_a_plugins_warm_instances() {
        let runtime = PluginRuntime::new().unwrap();
        let mut pool = InstancePool::new(runtime, Limits::default(), 4);
        let id = plugin_id();

        let instance = pool.checkout(&id, warm).unwrap();
        pool.release(&id, instance);
        pool.evict(&id);
        assert_eq!(pool.warm_count(&id), 0);

        pool.checkout(&id, warm).unwrap();
        assert_eq!(pool.misses(), 2);
    }
}
