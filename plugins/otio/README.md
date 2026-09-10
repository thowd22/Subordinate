# OpenTimelineIO interchange

Two first-party plugins that read and write [OpenTimelineIO][otio] JSON.
Interchange lives in plugins (docs/PLAN.md §3), and OTIO is the target the
project model was shaped around, so the mapping is nearly one to one.

| Crate           | Plugin id                      | World      | What it does                                    |
| --------------- | ------------------------------ | ---------- | ----------------------------------------------- |
| `otio-importer` | `com.subordinate.otio-import`  | `importer` | Reads an `.otio` file into media and a sequence |
| `otio-exporter` | `com.subordinate.otio-export`  | `command`  | Writes one sequence out as `.otio`              |
| `otio-core`     | —                              | —          | The schema and both conversions, host-testable  |

The exporter is a `command` plugin rather than an `exporter` one on purpose:
the `exporter` world contributes *encoder presets* and a post-encode hook, and
writing an OTIO document encodes nothing.

## Building and installing

```sh
cargo build --release --target wasm32-wasip2 --workspace   # from this directory
cp target/wasm32-wasip2/release/otio_importer.wasm otio-importer/plugin.wasm
cp target/wasm32-wasip2/release/otio_exporter.wasm otio-exporter/plugin.wasm
subordinate-cli plugin install otio-importer
subordinate-cli plugin install otio-exporter
```

`plugin install` takes either a built `.wasm` — finding the `plugin.toml`
beside it or above it — or a plugin directory holding a manifest and one
component. These three crates share one workspace, so `target/` sits at the
workspace root rather than inside either component's crate, and the directory
form with the component copied in beside its manifest is the one that finds the
right manifest for each.

Each component has its own `plugin.toml` beside its `Cargo.toml`: the importer
asks for read access to the open project's folder, the exporter for write
access, and neither asks for the network.

## Exporting

```sh
subordinate-cli plugin run com.subordinate.otio-export \
    --args '{"sequence": "Edit", "path": "/project/edit.otio"}'
```

Both arguments are optional. Without `sequence` the first sequence is exported;
without `path` the document comes back in the result instead of being written.
`path` lives inside the folder the manifest granted, which the host mounts at
`/project`.

## What crosses

| Subordinate             | OTIO                                        |
| ----------------------- | ------------------------------------------- |
| `Sequence`              | `Timeline`, whose `tracks` is a `Stack`     |
| `Track` (video / audio) | `Track` with `kind` `Video` / `Audio`       |
| muted track             | `enabled: false`                            |
| `Clip`                  | `Clip` with an `ExternalReference`          |
| `Gap`                   | `Gap`                                       |
| `Transition::Crossfade` | `Transition`, `SMPTE_Dissolve`              |
| sequence marker         | `Marker` on the `Stack`                     |
| clip marker             | `Marker` on the `Clip`, note as `comment`   |
| canvas, audio rate      | `metadata.subordinate` (OTIO stores neither) |

Times cross exactly. OTIO serialises a `RationalTime` as two JSON numbers, so
`otio-core::time` is the one place a float exists: it writes `24000/1001` as
OTIO writes it and recognises it on the way back in, rather than approximating
it.

## What does not: effects

**Effects are not exported, and effects are not imported.** That covers both
plugin effects and the fixed per-clip parameters the MVP has — opacity,
transform, gain, and the two fades. OTIO's `Effect` is a schema name plus an
untyped parameter bag with no agreed vocabulary for any of them, so an export
that invented names would import into other applications as nothing at all,
while looking as though the look had travelled. The cut survives a round trip;
the look does not. Re-grade after importing.

Also not carried:

- **Track locking**, which OTIO has no field for. It rides in the metadata on
  the way out and is not read back.
- **Marker colour**, which OTIO has and the MVP model does not (decision-3).
- **Subtitle tracks** and any transition other than a crossfade: an import
  skips the first and approximates the second, and says so in the log.
- **Nested stacks**, which the model does not have: a document with a stack
  inside a track is refused as `otio.invalid_document` rather than silently
  flattened.
- **Clips with no file behind them** — a `GeneratorReference` such as
  Kdenlive's colour clip, or a `MissingReference` — which import as a gap of
  the same length so that everything after them stays put.
- **Absolute media paths.** A project stores paths relative to its own folder,
  so an absolute path written by another machine is reduced to its file name
  and relinked like any other missing media.

Every one of those is reported: an import logs one line per thing it dropped or
guessed.

## Tests

`cargo test` runs everything on the host triple, because all the conversion
lives in `otio-core`:

- `tests/roundtrip.rs` exports `fixtures/project.json`, compares it against the
  committed golden `fixtures/roundtrip.otio` (re-record with
  `OTIO_UPDATE_FIXTURES=1`), and imports the result back.
- `tests/kdenlive.rs` imports `fixtures/kdenlive.otio`, a timeline in the shape
  Kdenlive's native OTIO export writes — `Clip.2` media reference maps, track
  source ranges, absolute `target_url`s, guides as one-frame stack markers,
  mixes as `SMPTE_Dissolve` transitions, a colour generator and a subtitle
  track.

[otio]: https://opentimelineio.readthedocs.io/
