#!/usr/bin/env python3
"""The clicks-only desktop flow (TASK-139 AC 4).

A person's session, performed by the harness: click the media bin's `Import...`
button, choose the baked clip in the **native** file dialog, drag it onto the
timeline, scrub the playhead and press `Ctrl+K`, then pin the vendor encoder in
the export panel and press `Export`. Nothing here asks the application to do
anything - every gesture is a real pointer move, a real button press or a real
keystroke on the desktop.

The Command API is used, but only ever to **read**: after each gesture the flow
asks the running editor what the project now looks like and asserts on the
answer. :class:`ReadOnly` refuses any method that is not a read, so a mutation
that crept into this flow would fail the run rather than quietly do the work
the mouse was supposed to do.

    FLOW_OUT=/tmp/flow python3 scripts/desktop/flow_clicks.py

It takes the same environment as `flow_mcp.py`, plus `FLOW_LAYOUT`, a JSON
object overriding where in the window the timeline and its ruler are (see
`LAYOUT` below).
"""

from __future__ import annotations

import json
import os
import platform
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from subdesktop import flow as flowlib  # noqa: E402
from subdesktop.mcp import Bridge, McpError  # noqa: E402
from subdesktop.session import DesktopError, Rect  # noqa: E402

WINDOW = "Subordinate"

#: Where the timeline lives inside the editor window, as fractions of it.
#:
#: The dock is a layout, not an accessibility tree: its panels have names, but
#: the timeline canvas is one widget with no named children, so a drop and a
#: scrub have to land at a point. Everything that *is* a named control - the
#: Import button, the encoder picker, the Export button - is found by name.
#: Fractions of the window rectangle for the places with nothing to name.
LAYOUT = {
    "timeline_drop": [0.55, 0.78],
    "ruler": [0.62, 0.66],
}

#: Pixel offsets from the window's top-left, used only when the accessibility
#: tree is not there to be asked. The media bin's controls sit at the top-left
#: of the window and stay put when it is resized, so an offset survives where a
#: fraction would not. Override with FLOW_LAYOUT.
OFFSETS = {
    "Import": [35, 98],
    "row": [180, 178],
}

#: The only methods this flow may call: reads, and nothing else.
READS = {
    "project.get",
    "project.revision",
    "project.list_sequences",
    "project.settings",
    "media.list",
    "timeline.get_state",
    "history.get",
    "playback.status",
    "system.list_methods",
}


class ReadOnly:
    """A bridge that answers reads and refuses everything else."""

    def __init__(self, bridge: Bridge) -> None:
        self._bridge = bridge

    def __call__(self, method: str, arguments: dict | None = None):
        if method not in READS:
            raise AssertionError(
                f"{method} is not a read; the clicks flow may not act through MCP"
            )
        return self._bridge.call(method, arguments)


def _env(name: str, default: str) -> str:
    return os.environ.get(name) or default


#: The same role under each platform's spelling. UI Automation says
#: `TabItem` and `Button`; AT-SPI says `page tab` and `push button`.
TAB_ROLES = ("TabItem", "page tab", "Tab", "tab")
BUTTON_ROLES = ("Button", "push button")


def _by_role(session, name: str, roles, *, timeout: float = 30.0):
    """The control with this name and one of these roles.

    Roles matter where a name is used twice: the export panel is a *tab* named
    `Export` holding a *button* named `Export`, and clicking the wrong one
    either does nothing or starts a render before the encoder is pinned.
    """
    for role in roles:
        try:
            return session.find(name, role=role, timeout=timeout / len(roles))
        except DesktopError:
            continue
    return session.find(name, timeout=timeout)


def locate(session, window, name: str, key: str, offsets: dict, *, timeout: float = 60.0):
    """The control called `name`, or the offset that stands in for it.

    Names first, always: they are what the application itself publishes. The
    offset is the documented fallback for a session whose accessibility bus is
    not there - on Linux that is the difference between a machine with
    `at-spi2-core` running and one without - and the flow records which of the
    two it used, so a run that fell back says so rather than looking the same.
    """
    try:
        return session.find(name, timeout=timeout), "name"
    except DesktopError as error:
        offset = offsets.get(key)
        if not offset:
            raise
        print(f"    (no accessibility tree for {name!r}: {error})", flush=True)
        return (window.rect.x + int(offset[0]), window.rect.y + int(offset[1])), "offset"


def run(run: flowlib.Run) -> None:
    out = run.out
    session = run.session
    work = Path(_env("FLOW_WORK", str(out / "project")))
    media = Path(_env("FLOW_MEDIA", "/opt/subordinate/test-media/meld-4k60-excerpt-2min.mkv"))
    editor = _env("FLOW_EDITOR", "subordinate")
    cli = _env("FLOW_CLI", "subordinate-cli")
    mcp = _env("FLOW_MCP", "subordinate-mcp")
    encoder = _env("FLOW_ENCODER", "nvh264enc")
    discoverer = _env("FLOW_DISCOVERER", "subordinate-gst-discoverer")
    layout = dict(LAYOUT)
    offsets = dict(OFFSETS)
    override = json.loads(os.environ.get("FLOW_LAYOUT") or "{}")
    layout.update({k: v for k, v in override.items() if k in LAYOUT})
    offsets.update({k: v for k, v in override.items() if k in OFFSETS})

    with run.step("stage the project folder") as step:
        staged = flowlib.stage(work, media, cli)
        step.note(project=str(staged.project), media=staged.media_relative)

    app_log = out / "editor.log"
    with run.step("launch the editor on the desktop") as step:
        started = session.launch(
            [editor, str(staged.project)],
            log=app_log,
            cwd=str(staged.directory),
            env={"SUBORDINATE_LOG": "info,sub_export=debug"},
        )
        window = session.wait_for_title(WINDOW, timeout=180)
        # Maximized first: a window bigger than the screen hides the timeline
        # behind the taskbar, and a drop aimed at it lands on the desktop
        # (run 34672165182).
        window = session.maximize(window)
        endpoint = flowlib.wait_for_command_api(session, app_log)
        session.activate(window)
        step.note(pid=started.pid, window=window.as_dict(), endpoint=endpoint)

    with run.step("connect a read-only bridge to the editor") as step:
        bridge = Bridge(
            mcp,
            cwd=staged.directory,
            env={"SUBORDINATE_MCP_NO_LAUNCH": "1"},
            require_editor=True,
            log=out / "mcp-reads.log",
        )
        step.note(connection=bridge.connection)
    read = ReadOnly(bridge)
    try:
        with run.step("the editor is the one being read") as step:
            before = read("media.list", {})
            step.note(connection=bridge.connection, media=len(_items(before)))
            if _items(before):
                raise AssertionError("the staged project should start with no media")

        with run.step("click Import... in the media bin") as step:
            control, how = locate(session, window, "Import", "Import", offsets)
            step.note(control=getattr(control, "as_dict", lambda: control)(), located_by=how)
            session.click(control)
            dialog = _wait_for_dialog(session, window, timeout=60)
            step.note(dialog=dialog)

        with run.step("choose the clip in the native file dialog") as step:
            _choose_file(session, staged.media)
            listed = _wait_until(
                lambda: _items(read("media.list", {})),
                lambda items: len(items) == 1,
                timeout=120,
                what="one media item in the bin",
            )
            step.note(media=[_name(item) for item in listed])

        with run.step("drag the clip from the bin onto the timeline") as step:
            session.activate(window)
            tree = [control.as_dict() for control in session.controls()]
            (out / "controls-timeline.json").write_text(json.dumps(tree, indent=1))
            row, how = locate(session, window, staged.media.stem[:12], "row", offsets)
            drop, lane = _lane_point(session, window, layout)
            step.note(
                row=getattr(row, "as_dict", lambda: row)(),
                located_by=how,
                drop={"x": drop[0], "y": drop[1]},
                lane=lane,
            )
            session.drag(row, drop)
            clips = _wait_until(
                lambda: _clips(read, staged.sequence),
                lambda found: len(found) >= 1,
                timeout=60,
                what="a clip on the timeline",
            )
            step.note(clips=len(clips))

        with run.step("scrub the playhead, then press Ctrl+K") as step:
            ruler = _ruler_point(session, window, layout)
            step.note(ruler={"x": ruler[0], "y": ruler[1]})
            session.click(ruler)
            position = _wait_until(
                lambda: read("playback.status", {}),
                lambda status: _units(status) > 0,
                timeout=30,
                what="a playhead somewhere other than zero",
            )
            step.note(scrubbed_to=position)
            session.key("ctrl+k")
            clips = _wait_until(
                lambda: _clips(read, staged.sequence),
                lambda found: len(found) >= 2,
                timeout=60,
                what="two clips after the cut",
            )
            step.note(clips=len(clips))

        output = staged.directory / "clicks-export.mp4"
        with run.step("open the export panel") as step:
            session.activate(window)
            tab = _by_role(session, "Export", TAB_ROLES, timeout=60)
            session.click(tab)
            time.sleep(1)
            tree = [control.as_dict() for control in session.controls()]
            (out / "controls-export.json").write_text(json.dumps(tree, indent=1))
            step.note(tab=tab.as_dict(), controls=len(tree))

        with run.step("limit the render to the first seconds") as step:
            # A clip dragged out of the bin is the whole two-minute source, and
            # rendering all of it would cost more GPU minutes than the rest of
            # the flow together. `In to out` with a small Out is the panel's
            # own way to say so. Best effort: a flow that cannot set it renders
            # the lot rather than failing over a convenience.
            try:
                session.click(session.find("In to out", timeout=20))
                out_label = session.find("Out", timeout=20)
                # An egui DragValue becomes a text field when it is clicked
                # into, and it sits immediately right of its label.
                session.click(
                    (out_label.rect.x + out_label.rect.width + 30, out_label.rect.center[1]),
                    double=True,
                )
                session.key("ctrl+a")
                session.type_text("120")
                session.key("Return")
                step.note(range="in to out", out_frame=120)
            except DesktopError as error:
                step.note(range="whole sequence", why=str(error)[:200])

        with run.step("type the output path and pin the vendor encoder") as step:
            # The output is a text field beside a `File` label, so it is typed
            # rather than chosen in a save dialog. The field itself has no name
            # of its own; the label does, and the field is immediately right of
            # it.
            label = session.find("File", timeout=60)
            session.click((label.rect.x + label.rect.width + 60, label.rect.center[1]))
            session.key("ctrl+a")
            session.type_text(str(output))
            # `ComboBox::from_label("Encoder")` draws the label beside the
            # button and the *selection* on it, so the button to click is the
            # one reading `Automatic` (export_panel.rs).
            picker = session.find("Automatic", timeout=30)
            session.click(picker)
            choice = session.find(encoder, timeout=30)
            session.click(choice)
            step.note(encoder=encoder, picker=picker.as_dict(), file_label=label.as_dict())

        with run.step("click Export and wait for the file") as step:
            session.click(_by_role(session, "Export", BUTTON_ROLES, timeout=30))
            _wait_until(
                lambda: output.exists() and output.stat().st_size,
                lambda size: bool(size),
                timeout=600,
                what="the exported file",
            )
            # The editor writes the file as it goes, so wait for it to settle.
            _settle(output)
            probe = subprocess.run(
                [discoverer, str(output)], capture_output=True, text=True, check=False
            )
            text = probe.stdout + probe.stderr
            (out / "export-probe.txt").write_text(text)
            used = _encoder_used(app_log)
            step.note(bytes=output.stat().st_size, encoder=used or "(not logged)")
            if "H.264" not in text and "h264" not in text.lower():
                raise AssertionError(f"the export is not H.264:\n{text[:800]}")
            if used != encoder:
                raise AssertionError(
                    f"the export ran on {used!r}, not the vendor encoder {encoder!r}"
                )

        with run.step("the edits are in the window's own undo stack") as step:
            history = read("history.get", {})
            step.note(history=json.dumps(history)[:400])
            if not json.dumps(history):
                raise AssertionError("the editor reported no history at all")
    finally:
        bridge.close()
        session.stop(started)


# ---------------------------------------------------------------- the lanes


def _lane_point(session, window, layout) -> tuple[tuple[int, int], str]:
    """Where to drop a video clip: the middle of the V1 lane.

    The lane itself is canvas with nothing to name, but its **header** is a
    label the application publishes - `V1`, from the track's own name - and the
    lane is the band of pixels beside it. Aiming at the header's own row is
    what makes this a drop on the video track rather than on the audio track
    below it: a video clip dropped on A1 is refused, which looks exactly like a
    drag that never happened (run 34673633121).
    """
    try:
        header = session.find("V1", timeout=20)
        x = window.rect.x + int(window.rect.width * 0.5)
        return (x, header.rect.center[1]), "the V1 header's row"
    except DesktopError:
        return window.rect.point(*layout["timeline_drop"]), "a fraction of the window"


def _ruler_point(session, window, layout) -> tuple[int, int]:
    """Where to click to scrub: the ruler, which is just above the first lane."""
    try:
        header = session.find("V1", timeout=10)
        x = window.rect.x + int(window.rect.width * 0.35)
        return (x, header.rect.y - 24)
    except DesktopError:
        return window.rect.point(*layout["ruler"])


# ----------------------------------------------------------------- the dialog


def _wait_for_dialog(session, window, *, timeout: float) -> dict:
    """Wait for the native file chooser the Import button opens."""
    deadline = time.monotonic() + timeout
    last = ""
    while time.monotonic() < deadline:
        if platform.system() == "Windows":
            found = _windows_dialog(session, window)
        else:
            found = _x11_dialog(session, window)
        if found:
            return found
        last = "nothing yet"
        time.sleep(1)
    raise DesktopError(
        f"the Import button opened no file dialog within {timeout:g}s ({last}). "
        "On Linux the editor's file dialog is an XDG desktop portal: the session "
        "needs xdg-desktop-portal with a backend and a session bus."
    )


def _windows_dialog(session, window) -> dict | None:
    for candidate in session._desktop.windows():  # noqa: SLF001 - backend detail
        try:
            if str(candidate.handle) == window.handle:
                continue
            title = candidate.window_text() or ""
            cls = candidate.class_name() or ""
        except Exception:  # noqa: BLE001
            continue
        if cls == "#32770" or "Open" in title or "Import" in title:
            return {"title": title, "class": cls}
    return None


def _x11_dialog(session, window) -> dict | None:
    found = subprocess.run(
        ["xdotool", "search", "--name", "(Import|Open|Select|Choose|File)"],
        capture_output=True,
        text=True,
        check=False,
    )
    for window_id in found.stdout.split():
        if window_id == window.handle:
            continue
        title = subprocess.run(
            ["xdotool", "getwindowname", window_id],
            capture_output=True,
            text=True,
            check=False,
        ).stdout.strip()
        if title and title != window.title:
            subprocess.run(
                ["xdotool", "windowactivate", "--sync", window_id], check=False
            )
            return {"title": title, "window": window_id}
    return None


def _choose_file(session, path: Path) -> None:
    """Type the file's path into whichever native chooser came up."""
    if platform.system() == "Windows":
        # The common item dialog opens with the File name box focused.
        session.type_text(str(path))
        session.key("Return")
    else:
        # GTK's chooser: Ctrl+L opens the location bar, and the path goes in it.
        session.key("ctrl+l")
        session.type_text(str(path))
        session.key("Return")
    time.sleep(2)


# ------------------------------------------------------------------- reading


def _items(listed) -> list:
    if isinstance(listed, dict):
        for key in ("media", "items"):
            if isinstance(listed.get(key), list):
                return listed[key]
    return listed if isinstance(listed, list) else []


def _name(item) -> str:
    return item.get("name", "") if isinstance(item, dict) else str(item)


def _clips(read, sequence: str) -> list:
    state = read("timeline.get_state", {"sequence": sequence})
    tracks = _tracks(state)
    for track in tracks:
        items = track.get("items") or track.get("children") or []
        clips = [item for item in items if isinstance(item, dict) and "clip" in item]
        if clips:
            return clips
    return []


def _tracks(state) -> list:
    if isinstance(state, dict):
        if isinstance(state.get("tracks"), list):
            return state["tracks"]
        for value in state.values():
            found = _tracks(value)
            if found:
                return found
    return []


def _units(status) -> int:
    """The playhead's unit count, however the transport reports it."""
    if isinstance(status, dict):
        for key in ("position", "playhead", "time"):
            value = status.get(key)
            if isinstance(value, dict) and "value" in value:
                return int(value["value"])
        for value in status.values():
            found = _units(value)
            if found:
                return found
    return 0


def _wait_until(read, predicate, *, timeout: float, what: str):
    """Poll `read` until `predicate` likes the answer, or say what was missing."""
    deadline = time.monotonic() + timeout
    answer = None
    error = None
    while time.monotonic() < deadline:
        try:
            answer = read()
            if predicate(answer):
                return answer
        except (McpError, AssertionError) as caught:
            error = caught
        time.sleep(1)
    raise AssertionError(
        f"never saw {what} within {timeout:g}s; last answer: "
        f"{json.dumps(answer, default=str)[:500]}{f' ({error})' if error else ''}"
    )


def _settle(path: Path, *, quiet: float = 4.0, timeout: float = 600.0) -> None:
    """Wait until a file has stopped growing."""
    deadline = time.monotonic() + timeout
    size = -1
    unchanged = 0.0
    while time.monotonic() < deadline:
        current = path.stat().st_size
        unchanged = unchanged + 1 if current == size else 0
        size = current
        if unchanged >= quiet:
            return
        time.sleep(1)


def _encoder_used(log: Path) -> str | None:
    if not log.exists():
        return None
    for line in log.read_text(encoding="utf-8", errors="replace").splitlines():
        if "export pipeline started" in line or "video_encoder" in line:
            for part in line.replace('"', " ").replace("=", " ").split():
                if part.endswith("enc") and part not in {"video_encoder", "audio_encoder"}:
                    return part
    return None


if __name__ == "__main__":
    sys.exit(flowlib.main(run, "Clicks flow"))
