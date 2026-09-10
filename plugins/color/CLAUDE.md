# Colour Grade

The first-party reference plugin for the `effect` world of
`subordinate:plugin@0.1.0` — and the template to copy when writing one. It
tints, exposes and saturates a picture with a single WGSL shader.

- Plugin id: `com.subordinate.color` — the install directory, the log tag and
  the capability grant all key on it.
- SDK: `subordinate-sdk`, `default-features = false, features = ["effect"]`.
  Exactly one world feature may be on.
- Target: `wasm32-wasip2`. The Rust toolchain emits a component directly, so
  there is no `cargo component` or `wasm-tools` step.
- Licence: MIT OR Apache-2.0, like the WIT and the SDK, so this code may be
  copied into a plugin under any licence (decision-2).

## The interface

```rust
fn describe() -> EffectDesc
```

One export, called once when the host loads the plugin and again after a hot
reload — never per frame. `EffectDesc` carries the parameters the inspector
shows, the WGSL source and the name of its fragment entry point. A WASM guest
never touches the GPU (decision-6): the host derives a uniform struct from the
parameter list, prepends its own bindings and full-screen vertex stage,
compiles the module, caches it by hash and runs it per clip in the compositor.

The plugin has no say in a clip's parameter *values*. Those live in the project
model and are edited by ordinary undoable commands, which is why this plugin
imports nothing from the Command API and holds no state at all.

## The three files

| File | What it is |
| --- | --- |
| `src/grade.rs` | The parameter table and a CPU reference for what the shader does. No WIT, no bindings — plain Rust, unit-tested on the host triple. |
| `src/effect.wgsl` | The shader: one `@fragment fn fs_main(in: VsOut) -> @location(0) vec4<f32>`. |
| `src/lib.rs` | The lift from the first to an `effect-desc`, plus `export!`. |

That split is the template's real content. Keeping the parameter table and the
reference maths out of the bindings means:

- one place declares a parameter, so its id, label, range and default cannot
  drift between the inspector, the uniform block and the shader;
- the maths is testable without a GPU or a component runtime;
- the host's golden test, `crates/sub-render/tests/color_plugin_golden.rs`,
  reads `src/grade.rs` with `#[path]` and `src/effect.wgsl` with
  `include_str!`, so it renders *this* shader with *these* parameters and
  compares the readback with `Grade::apply`. Rename a parameter or change the
  order of the grade in one place only and that test fails.

## The shader's contract

The host prepends the prelude, so `src/effect.wgsl` declares none of it:

- `params: EffectParams` — one member per declared parameter, named by its
  `id`, in declaration order, each in its own 16-byte slot. `params.exposure`
  is an `f32`, `params.tint` a `vec4<f32>`.
- `source: texture_2d<f32>` with `source_sampler` — the clip's picture, an
  sRGB texture, so a sample is already in linear light. The sRGB target
  encodes again on store; do the maths in between and touch neither curve.
- `VsOut` — `position` and `uv`, from the host's `vs_effect`. A plugin may not
  declare a vertex stage of its own or bind any resource itself.

The parameter ids are WGSL identifiers (`[a-z][a-z0-9_]*`) because they *are*
the member names. Alpha is handed on untouched: an effect that changed coverage
would change what the compositor blends underneath.

## Capabilities

`plugin.toml` asks for `shaders = true` and nothing else — the host compiles a
plugin's WGSL only where that grant exists. No filesystem, no network: an
undeclared capability is a denied one.

## The loop

```
cargo build --release --target wasm32-wasip2
subordinate-cli plugin install ./target/wasm32-wasip2/release/subordinate_plugin_color.wasm --dev
subordinate-cli plugin test com.subordinate.color
```

`--dev` links the component to these sources, so a running host watches the
file and hot-reloads it: rebuild and the editor picks up the new shader without
a restart. `cargo test` here runs the grade's own tests on the host triple, and
`cargo test -p sub-render --test color_plugin_golden` renders the shader.

## House rules

- Never compute timeline positions in floating point. `RationalTime` is exact;
  seconds are not.
- Never hold the project model: an effect plugin does not edit it at all, and a
  world that does edits it through the Command API so every change lands on the
  host's undo stack.
- Keep `plugin.toml` truthful: the host refuses anything it declares that the
  component does not export.
- Defaults are the identity: an effect dropped on a clip should change no pixel
  until a control is moved.
