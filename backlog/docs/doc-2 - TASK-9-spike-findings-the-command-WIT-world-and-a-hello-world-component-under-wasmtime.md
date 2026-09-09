---
id: doc-2
title: >-
  TASK-9 spike findings: the command WIT world and a hello-world component under
  wasmtime
type: specification
created_date: '2026-09-09 20:17'
updated_date: '2026-09-09 20:20'
tags:
  - spike
  - plugins
  - wit
---
Spike for TASK-9 (docs/PLAN.md §6, decision-6). The durable output is
`wit/subordinate-plugin.wit`; the code in `spikes/wasm-command-world` is
throwaway proof that the WIT links, runs and can be stopped. Every number below
was measured on the machine described under *Environment*; nothing is estimated.

## What the spike does

`wit/subordinate-plugin.wit` declares `subordinate:plugin@0.1.0` with:

- `interface types` — the `error` record (a `SubError` in WIT clothing: stable
  `code`, `message`, `details` list) and `rational-time`;
- `interface command-api` — the host import: `invoke`, `query`, `playhead`,
  `log`;
- `world command` — imports `command-api`, exports
  `run: func(project: string, args: string) -> result<string, error>`.

The spike crate is a wasmtime host implementing `command-api` against
`FixtureProject` (a stand-in for `sub-model`, which does not exist yet), plus
two guest components:

| guest | what it does | proves |
| --- | --- | --- |
| `marker` | reads `playhead`, then invokes `sequence.add_marker` | the host-import round trip and the error path |
| `looper` | never returns | fuel and epoch termination |

Seven tests cover it; `cargo test -p spike-wasm-command-world` runs them.

## Finding 1: cargo component is no longer part of the toolchain

The acceptance criterion said "built with `cargo component`". That tool is not
needed and was not used. Since Rust 1.82 the `wasm32-wasip2` target emits a
**component**, not a core module, so a guest is an ordinary crate with
`crate-type = ["cdylib"]`, a `wit-bindgen` dependency, a
`wit_bindgen::generate!({ path: "../../../../wit", world: "command" })` call and
an `export!(Plugin)`. Then `cargo build --release --target wasm32-wasip2`
produces the `.wasm` the host loads. No `cargo component`, no
`wasm-tools component new`, no adapter module to carry around.

This matters well beyond the spike: the plugin scaffold (TASK-90) and the
agent-facing build story (TASK-102) get one fewer tool to install, and
`rustup target add wasm32-wasip2` is the whole setup. Consequence for the SDK
(TASK-89): the SDK is a plain library crate re-exporting `wit-bindgen` plus
helpers, not a cargo subcommand.

## Finding 2: guests must be their own workspaces, built by a build script

A guest cannot be a workspace member: `cargo clippy --workspace --all-targets`
would try to build it for the host triple and fail, and the workspace lints do
not apply to a `cdylib` for another target. Each guest therefore carries an
empty `[workspace]` table and is built from `build.rs` into `OUT_DIR`, with the
paths handed to the tests via `cargo::rustc-env`.

Two things the nested build gets wrong if you let it:

- **Inherited flags.** This machine sets `RUSTFLAGS` with host `-L` paths for
  GStreamer. Passed through to a `wasm32-wasip2` build they are at best noise
  and at worst a link error. The build script clears `RUSTFLAGS`,
  `CARGO_ENCODED_RUSTFLAGS`, `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`,
  `CARGO_BUILD_TARGET` and `CARGO_MAKEFLAGS` before spawning cargo.
- **A missing target.** If `wasm32-wasip2` is not installed, the script emits
  `cfg(no_wasm_guests)` and the four wasm tests compile out, so the crate still
  builds on a machine without it. CI must therefore run
  `rustup target add wasm32-wasip2`, or those tests silently do not exist.

## Finding 3: rate cannot be a single integer

The first draft of `rational-time` was `{ value: s64, rate: u32 }` — ticks per
second. That is wrong for exactly the rates that matter: 23.976 fps is
24000/1001 and 29.97 is 30000/1001, neither of which is a whole number of ticks
per second, so a plugin round-tripping a playhead would drift. The record now
carries the rate as a fraction of `rate-numerator` over `rate-denominator`,
which maps one-to-one onto `sub_time::RationalTime` and `Rational`. The
`marker_plugin_edits_the_project_through_the_host_import` test asserts the
marker lands exactly on the playhead it read, with no rounding.

## Finding 4: JSON for params, WIT for shape

`invoke` and `query` take `method: string` and `params: string`, where `params`
is a JSON document — deliberately not a WIT variant per command. Reasons:

- the Command API schema (TASK-5) is the single source of truth, and a new
  method must not force a WIT version bump and a shim;
- a plugin and an external MCP client then have provably the same reach, which
  is the point of the one-Command-API decision;
- WIT variants for some fifty commands would be unreadable and would churn
  constantly.

The cost is that argument errors are runtime errors rather than link errors. The
`error` record makes them legible: the marker guest called with `{}` comes back
as `plugin.invalid_argument` with the message intact, asserted in
`marker_plugin_error_crosses_back_as_a_sub_error`.

`error.details` is a `list<detail>` of string pairs whose values are JSON
fragments, because WIT has no map type and no dynamic JSON type. Round-tripping
`SubError -> WitError -> SubError` is lossless apart from `cause`, which is
deliberately not exposed: a cause chain is host-internal and may name paths a
sandboxed plugin should not see.

## Finding 5: fuel and epochs answer different questions — use both

Both work, and they are not interchangeable.

| | fuel | epoch deadline |
| --- | --- | --- |
| counts | executed instructions | wall-clock time |
| deterministic | yes — same input, same stop | no |
| catches a slow host call | no (host time is not fuel) | yes |
| needs | `Config::consume_fuel(true)`, `Store::set_fuel` | `Config::epoch_interruption(true)` plus a thread calling `Engine::increment_epoch` |

Measured: the looper guest with a 1 000 000-fuel budget stops having consumed
exactly 1 000 000 fuel; with a 200 ms deadline and a 5 ms ticker it stopped at
209.7 ms.

Recommendation for the real host (TASK-84): enable both. Fuel is the per-call
budget — reproducible, so a plugin test in the harness (TASK-91) fails the same
way on every machine. The epoch deadline is the backstop that also covers a
plugin wedged inside a host call. One ticker thread per process, not per call as
the spike does.

Three mechanics worth writing down:

1. Classify the stop with `err.downcast_ref::<wasmtime::Trap>()` and match
   `Trap::OutOfFuel` / `Trap::Interrupt`. Matching on the rendered message works
   today and will break on a wasmtime upgrade.
2. With `epoch_interruption` compiled in, a store whose deadline is never set
   traps immediately. When no wall-clock limit is wanted, set it to `u64::MAX`.
   The same applies to fuel: `set_fuel(u64::MAX)` for unlimited.
3. Do **not** install an `epoch_deadline_callback` returning
   `UpdateDeadline::Yield` on a synchronous store — yielding needs an async
   store. The default behaviour (trap) is what a runaway plugin should get.

## Finding 6: StoreLimits::instances(1) rejects every real plugin

One component is several core instances once the WASI adapter is linked in; a
ceiling of 1 fails instantiation with `instance count too high at 2`. The spike
uses 64 instances and a 64 MiB memory ceiling. The memory ceiling is the one
worth tuning per plugin capability (TASK-83); the instance count is a
denial-of-service guard, not a budget.

## Finding 7: the cost of a plugin call is not the thing to optimise

For the marker guest, one whole call — instantiate, read the playhead, invoke a
command, return:

| measurement | value |
| --- | --- |
| component size on disk | 69 796 bytes (`opt-level = "s"`, `strip = true`) |
| instantiation | 131 µs |
| fuel for the whole call | 8 161 |
| wall clock, cold `Component::from_file` excluded | 5.3 ms first call, about 0.2 ms warm |

8 161 fuel against a default budget of 50 000 000 says a per-call fuel budget
can be generous without being useless. Instantiation at about 131 µs means a
fresh instance per call is affordable for command plugins, which is the safer
design: no state survives a call, so a misbehaving plugin cannot poison the next
one. The looper guest is 14 040 bytes, so the floor for a component is around
14 KB and the extra 55 KB in the marker guest is `format!` and its panic
machinery — worth telling plugin authors about in the author guide (TASK-108).

## What this settles for the other WIT worlds

- `command-api` is the shared import; TASK-75 should add logging and project
  queries to this same interface rather than inventing a second one.
- Every fallible export returns `result<_, error>` with the `types.error`
  record. Do not let a world invent its own error type.
- Time crosses as `rational-time` with a fractional rate, never as seconds.
- The `command` world here is deliberately smaller than TASK-80: no menu or
  shortcut registration. Those must arrive as a new world or new exports, since
  changing `world command` itself breaks existing guests.

## Environment

- Linux 6.6.87 (WSL2), x86_64; no GPU needed for any of this.
- Rust 1.95.0, `wasm32-wasip2` target installed via rustup.
- wasmtime 48.0.1 (`component-model`, `cranelift`, `runtime`), wasmtime-wasi
  48.0.1 (`p2`), wit-bindgen 0.61.1 on the guest side.
- `cargo build` and `cargo test` at `CARGO_BUILD_JOBS=2` with other agents
  running, so the wall-clock figures are upper bounds.

## Reproducing

    rustup target add wasm32-wasip2
    cargo test -p spike-wasm-command-world -- --nocapture

The two `eprintln!` calls in the tests print the fuel and timing figures above.
