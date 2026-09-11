# Sample project

`demo.sub` is the ready-made project: open it to see the editor doing
something, and let CI render it to prove the same thing still works.

```bash
./scripts/get-sample-media.sh          # or scripts\get-sample-media.ps1 on Windows
cargo run -p subordinate -- examples/sample-project/demo.sub
```

The media is **not committed**. `scripts/get-sample-media.sh` (and its
PowerShell twin) downloads three CC0 clips into `media/`, each pinned by byte
size and SHA-256, and writes a `media/manifest.json` recording where every file
came from. A file whose bytes changed fails the fetch rather than quietly
changing what the project renders.

The clips are served from this project's own release,
[`sample-media-v1`](https://github.com/thowd22/Subordinate/releases/tag/sample-media-v1),
and checked against the `SHA256SUMS` published beside them as well as the pins
in the script. They were first published on Wikimedia Commons, and the credits
below keep those pages; nothing in the default fetch, and nothing in CI,
contacts Wikimedia. `--upstream` (PowerShell: `-Upstream`) fetches from the
original Commons URLs instead, for anyone who wants to re-verify provenance.

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

*Porters, wide* also carries an **applied effect**: the first-party colour
plugin `com.subordinate.color` (`plugins/color`), a stop of exposure with a
warm tint. The project stores only the reference — the plugin id and the values
that differ from the plugin's declared defaults — because the parameters and
the WGSL belong to the plugin and are read from it at load time. An effect
whose plugin is not installed keeps its values in the file and simply does not
run.

## Tests over it

- `cargo test -p sub-model --test demo_project` — the project builder, the
  golden bytes, the round trip, the path rules and the stored effect. No media
  needed.
- `cargo test -p sub-ui --test sample_project_render` — the render test: it
  decodes the real media, composites the layers, checks the crossfade blends
  and runs the grade the project applies, bound against `plugins/color`'s own
  declaration. Skips itself when the media has not been fetched or the machine
  has no wgpu adapter; CI sets `SUB_REQUIRE_SAMPLE_MEDIA=1` on all three OSes,
  which turns the missing-media skip into a failure, so a green run there is
  the proof that the project opens with every path resolved and no relink.

To change the project, edit the builder in
`crates/sub-model/tests/demo_project.rs` and regenerate:

```bash
SUB_UPDATE_GOLDEN=1 cargo test -p sub-model --test demo_project
```

## Media credits

All three files are dedicated to the public domain under
[CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/). No attribution
is required; it is given anyway.

Each row's source is the Commons page the file was taken from; all three are
mirrored as release assets under the names below.

| File | Title | Author | Original source |
| --- | --- | --- | --- |
| `media/porters-paris-1921.webm` | *Ancienne et nouvelle tenue des porteurs des Pompes Funèbres de la Ville de Paris* (1921) | Le Saint Lucien | [Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Ancienne_et_nouvelle_tenue_des_porteurs_des_Pompes_Fun%C3%A8bres_de_la_Ville_de_Paris_-_AI49294.webm) |
| `media/crowned-pigeon.webm` | *Victorian Crowned Pigeon fighting* (2023) | Designism | [Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Victorian_Crowned_Pigeon_fighting.webm) |
| `media/soneros-en-xalapa.webm` | *Soneros en Xalapa* (2013) | Koffermejia | [Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Soneros_en_Xalapa.webm) |

The three between them cover more than one shape of source: VP9 in WebM at
720x576 with a 16:15 sample aspect, at 1010x616 square-pixel, and a 352x288
mono-audio clip at 25/3 fps.
