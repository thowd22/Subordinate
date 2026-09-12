# Desktop automation harness

Drive the real Subordinate window on a real desktop, on Linux and on Windows,
with the same seven verbs — and photograph what happened (TASK-139).

`egui_kittest` (docs/DEVELOPMENT.md, "UI tests") exercises panels in isolation
and is where regressions belong. This is the other kind of test: the whole
application on a machine with a GPU, a window manager and a pointer, doing what
a person or an agent would do end to end. It runs on the two desktop CI images
— `infra/images/linux-desktop` and `infra/images/windows-desktop` — and the
flows below are its two jobs in `.github/workflows/desktop-flows.yml`.

## What is here

| | |
| --- | --- |
| `subdesktop/` | the harness: `session.py` (the verbs), `linux.py` (X11), `windows.py` (UI Automation), `mcp.py` (an MCP stdio client), `flow.py` (staging, steps, verdicts) |
| `example.py` | the smallest flow: launch, read the tree, click `Import...`, close |
| `flow_mcp.py` | the MCP-only flow: an agent acts through `subordinate-mcp` alone |
| `flow_clicks.py` | the clicks-only flow: input only, asserted through read-only Command API calls |
| `windows/run-flow.ps1` | what carries a flow into the Windows console session |

## The command set

```python
from subdesktop import open_session

desktop = open_session("shots")                       # X11 or UI Automation
app = desktop.launch(["subordinate", "flow.sub"],     # detached, so it outlives us
                     log="editor.log")
window = desktop.wait_for_title("Subordinate")        # -> Window(handle, title, rect)
desktop.wait_for_log("editor.log", "render device ready on")
button = desktop.find("Import...")                    # by AccessKit name
desktop.click(button)                                 # a real pointer move and click
desktop.key("ctrl+k")                                 # one chord, xdotool spelling
desktop.type_text("/tmp/out.mp4")
desktop.drag(button, window.rect.point(0.55, 0.78))   # press, move in steps, release
desktop.screenshot("02-after-import.png")
desktop.stop(app)
```

| Verb | Linux | Windows |
| --- | --- | --- |
| `launch` | `Popen` in a session of its own | `Popen` with `DETACHED_PROCESS` |
| `wait_for_title` | `xdotool search --name` | `Desktop(backend="uia").windows()` |
| `wait_for_log` | shared: poll the file for a matching line | same |
| `find` / `controls` | AT-SPI (`pyatspi`) | UI Automation (`pywinauto`) |
| `click`, `key`, `type_text`, `drag` | `xdotool` | `pywinauto.mouse` / `keyboard` |
| `screenshot` | `scrot` | `System.Drawing.CopyFromScreen` |

Chords are spelled the xdotool way on both — `ctrl+k`, `Return`, `Escape` — and
the Windows backend translates them for `send_keys`.

Everything is also a command, so a shell or a PowerShell step can use it:

```sh
python3 -m subdesktop launch --log app.log -- subordinate flow.sub
python3 -m subdesktop wait-for-title Subordinate
python3 -m subdesktop controls --name Import      # dump the accessibility tree
python3 -m subdesktop click --name 'Import...'
python3 -m subdesktop key ctrl+k
python3 -m subdesktop screenshot 03-after-cut.png
```

Each subcommand prints JSON.

### Names, not pixels

egui publishes its widget tree through AccessKit; Windows exposes that as UI
Automation and Linux as AT-SPI. So a control is addressed by the name the
source gives it — `Import...` is `crates/sub-ui/src/media_bin.rs`, `Encoder`
and `Export` are `crates/sub-ui/src/export_panel.rs` — and a panel that moves
does not break the test. `find` matches on a name prefix, because labels carry
decoration a test should not have to spell.

A point is still accepted where there is genuinely nothing to name: the
timeline canvas is one widget with no named children, so a drop and a scrub
land at a fraction of the window rectangle (`LAYOUT` in `flow_clicks.py`,
overridable with `FLOW_LAYOUT`).

**On Linux this needs the accessibility bus.** AccessKit's Unix adapter
publishes the tree over AT-SPI, so a session without `at-spi2-core` has no tree
at all and `find` says so rather than pretending the button is missing. The
clicks job installs `at-spi2-core` and `python3-pyatspi` and runs the flow under
`dbus-run-session`; the same packages belong in the image once this settles.

## Writing a flow

`subdesktop.flow` turns a script into a sequence of named steps, photographs
the desktop after each one, and writes a verdict:

```python
from subdesktop import flow as flowlib

def my_flow(run):
    with run.step("launch the editor") as step:
        app = run.session.launch(["subordinate"], log=run.out / "editor.log")
        step.note(pid=app.pid)

if __name__ == "__main__":
    raise SystemExit(flowlib.main(my_flow, "My flow"))
```

Output lands in `$FLOW_OUT` (default `flow-out/`):

| | |
| --- | --- |
| `shots/NN-step-name.png` | one screenshot per step, in order |
| `result.json` | every step with its state, its seconds, its facts and its screenshot |
| `summary.md` | the same as a table, also appended to the job summary |
| `editor.log`, `mcp-*.log` | what the application and the bridge said |

`run.before_shot` is called just before each screenshot. A flow whose edits
arrive over a socket sets it to `session.nudge(window)`: egui paints only when
something asks it to, so on an idle desktop the window keeps showing the frame
it painted before the agent's edit, and the picture would be of a project that
has already changed (run 34672010010). A pointer move wakes it and mutates
nothing.

A failing step prints
`::error title=<flow>: <step>::step '<step>' failed; screenshot NN-step.png`,
so the job's annotation names the step and the artifact holds its picture.

`flowlib.stage()` is the staging both flows share: it makes a work folder,
**copies** the test clip into it (a media path is project-relative by model
rule, and `MediaPath::relative_to` resolves symlinks, so a link to
`/opt/subordinate/test-media` is refused with `model.invalid_path`) and writes a
project with `subordinate-cli new`.

## The two flows

Both take their paths from the environment, so they run against a local build
as well as against an image:

| Variable | Default |
| --- | --- |
| `FLOW_OUT` | `flow-out` |
| `FLOW_WORK` | `$FLOW_OUT/project` |
| `FLOW_MEDIA` | `/opt/subordinate/test-media/meld-4k60-excerpt-2min.mkv` |
| `FLOW_EDITOR`, `FLOW_CLI`, `FLOW_MCP` | `subordinate`, `subordinate-cli`, `subordinate-mcp` |
| `FLOW_DISCOVERER` | `subordinate-gst-discoverer` |
| `FLOW_PRESET`, `FLOW_ENCODER` | `youtube-1080p`, `nvh264enc` |

### `flow_mcp.py` — the agent's session

`project.new` and `project.open`, `media.import` of the baked clip,
`timeline.add_clip`, `timeline.split_clip`, `project.save`, then
`export.render` and `export.progress`, with a screenshot after every one and
the finished file probed with `gst-discoverer`. The bridge runs with
`SUBORDINATE_MCP_NO_LAUNCH=1` and `require_editor`, so every edit demonstrably
lands in the window on screen rather than in a headless engine.

The render is the exception, and deliberately so: the `export.*` family is
served by the process that owns the encoder (`docs/schema/host-api.json`), and
only `subordinate-cli serve` installs it — the editor's endpoint serves the
engine's methods and the plugin methods. So the flow saves the project the
window is holding and makes the last two calls through a second bridge on a
scratch instance, against the file it just wrote. Still MCP and nothing else.

### `flow_clicks.py` — the person's session

Click `Import...`, choose the clip in the **native** file dialog, drag it from
the bin onto the timeline, click the ruler to scrub, press `Ctrl+K`, type the
output path, pick `nvh264enc` in the `Encoder` picker and press `Export`. After
each gesture the flow reads the project back through the Command API and
asserts on it — one media item, then one clip, then a playhead off zero, then
two clips, then a file that probes as H.264 from the encoder that was pinned.

Nothing here *acts* through MCP: `ReadOnly` refuses any method that is not a
read, so a mutation smuggled into this flow fails the run.

On Linux the file dialog is rfd's XDG desktop portal backend (rfd 0.17 with
default features: neither `gtk3` nor `ashpd` is compiled in), so the session
needs `xdg-desktop-portal` with a backend behind it — the job installs
`xdg-desktop-portal-gtk` and sets `XDG_CURRENT_DESKTOP`. The chooser is a
window of its own: it is given the keyboard and moved into the pointer's reach
before anything is typed at it, and the clip is picked by double-clicking the
row carrying its name. On Windows it is the common item dialog, whose window
class is `#32770`, and the path is typed into the File name box it opens with.

Three things about these machines the harness has to work around, each of them
found the hard way and each of them a comment in the code:

| | |
| --- | --- |
| The pointer cannot reach the whole Linux screen | `xdotool mousemove` stops at x=448 on a 1920-wide screen, so `maximize()` measures `pointer_bounds()` and puts the window inside it |
| AccessKit publishes nothing until an AT asks | `enable_accessibility()` sets `org.a11y.Status`, which is what a screen reader does |
| The dock's tabs are painted, not published | the export panel's tab is found in the band above the Inspector's own text |

## Running them

Locally, against a build of your own:

```sh
cargo build --release -p subordinate -p subordinate-cli -p subordinate-mcp
FLOW_OUT=/tmp/flow \
FLOW_EDITOR=target/release/subordinate \
FLOW_CLI=target/release/subordinate-cli \
FLOW_MCP=target/release/subordinate-mcp \
FLOW_MEDIA=/path/to/a/clip.mkv \
FLOW_ENCODER=vah264enc \
python3 scripts/desktop/flow_mcp.py
```

In CI, `.github/workflows/desktop-flows.yml` runs all four jobs nightly and on
release tags through `hardware.yml`, and by hand:

```sh
gh workflow run desktop-flows.yml --ref main -f only=linux-clicks
# from a branch, where RunsOn cannot resolve a runner name yet:
gh workflow run desktop-flows.yml --ref my-branch \
  -f linux_runner=ami=ami-08dbaf8367717a9a3/family=g4dn.xlarge/spot=false
```

## Fresh-project regression coverage

Both routine flows launch the editor without a project file. The clicks flow
checks each track context menu on an empty timeline (undoing back to empty),
imports an external file through the native chooser before saving, and drops
it onto the empty timeline to create its first sequence and video track.
The MCP flow calls `project.new`, imports `{external: absolutePath}` media,
and creates the sequence and tracks through commands. Both retain their edit
and export checks and verify media references and timeline edits after the
first save and reopen. No staged project or copied source media masks first use.

`FLOW_REQUESTED_REF` identifies the checkout, `FLOW_TESTED_REF` the current
artifact, and `FLOW_INSTALLED_RELEASE` the image's unused baked application.
Hosted builders produce the editor, CLI, MCP bridge and focused test binaries
from the same SHA. Binary hashes and the SHA are checked before any test runs.
The Windows artifact includes its GStreamer runtime; Linux uses the matching
Ubuntu runtime. The image supplies the isolated desktop and test media only.

Routine hardware/nightly/release runs include noninteractive `box` and
`yodaddy` jobs. They execute the real egui application harness tests
`empty_timeline_drop`, `empty_track_menu`, `media_import_app`, and the MCP stdio
import/save/reopen regression. They create temporary configuration and endpoint
directories, inject no desktop input, and do not open the user's editor. Missing
GPU adapters and missing test filters fail rather than silently pass. Use
`only=regressions` for just these two free runner jobs (or `box`/`yodaddy` for
one); the native clicks/MCP flows continue on isolated paid desktop instances.
No compilation happens on paid instances or the user's machines.
