# Sample project

`demo.sub` is the ready-made project: open it to see the editor doing
something, and let CI render it to prove the same thing still works.

```bash
./scripts/get-sample-media.sh          # or scripts\get-sample-media.ps1 on Windows
cargo run -p subordinate -- examples/sample-project/demo.sub
```

The media is **not committed**. `scripts/get-sample-media.sh` (and its
PowerShell twin) downloads three CC0 clips from Wikimedia Commons into
`media/`, each pinned by URL, byte size and SHA-256, and writes a
`media/manifest.json` recording where every file came from. A file that changed
upstream fails the fetch rather than quietly changing what the project renders.

## What is in it

Two sequences:

- **Main cut** — 1280x720 at 25 fps. `V1` runs *Porters, wide* into
  *Pigeon, fight* through a one-second crossfade centred on the cut at frame
  150; `V2` lays *Soneros, corner* over the second half at 60 % opacity,
  scaled to 45 % and pushed into the top right; `A1` carries a music bed at
  −6 dB with fades at both ends. One sequence marker spans the dissolve and one
  clip marker sits on the wide shot.
- **Titles** — 1920x1080 at 30 fps: a title bed with fades either side and a
  sting under it.

Every media path is relative and forward-slashed (`media/…`), so the project
opens without a relink on Linux, Windows and macOS: the loader builds the
native path from that at load time. Every media item also records the content
hash of the pinned download, so media that is *not* the media this project was
authored against is caught rather than silently rendered.

The colour grade the sample demonstrates is applied by the render test rather
than stored in `demo.sub`: the project model carries no effect stack yet
(TASK-88). When it does, the grade moves into the project file.

## Tests over it

- `cargo test -p sub-model --test demo_project` — the project builder, the
  golden bytes, the round trip and the path rules. No media needed.
- `cargo test -p sub-ui --test sample_project_render` — the render test: it
  decodes the real media, composites the layers, checks the crossfade blends
  and runs the first-party `plugins/color` grade over a clip. Skips itself when
  the media has not been fetched or the machine has no wgpu adapter.

To change the project, edit the builder in
`crates/sub-model/tests/demo_project.rs` and regenerate:

```bash
SUB_UPDATE_GOLDEN=1 cargo test -p sub-model --test demo_project
```

## Media credits

All three files are dedicated to the public domain under
[CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/). No attribution
is required; it is given anyway.

| File | Title | Author | Source |
| --- | --- | --- | --- |
| `media/porters-paris-1921.webm` | *Ancienne et nouvelle tenue des porteurs des Pompes Funèbres de la Ville de Paris* (1921) | Le Saint Lucien | [Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Ancienne_et_nouvelle_tenue_des_porteurs_des_Pompes_Fun%C3%A8bres_de_la_Ville_de_Paris_-_AI49294.webm) |
| `media/crowned-pigeon.webm` | *Victorian Crowned Pigeon fighting* (2023) | Designism | [Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Victorian_Crowned_Pigeon_fighting.webm) |
| `media/soneros-en-xalapa.webm` | *Soneros en Xalapa* (2013) | Koffermejia | [Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Soneros_en_Xalapa.webm) |

The three between them cover more than one shape of source: VP9 in WebM at
720x576 with a 16:15 sample aspect, at 1010x616 square-pixel, and a 352x288
mono-audio clip at 25/3 fps.
