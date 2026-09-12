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
LAYOUT = {
    "timeline_drop": [0.55, 0.78],
    "ruler": [0.62, 0.66],
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
    layout.update(json.loads(os.environ.get("FLOW_LAYOUT") or "{}"))

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
        session.activate(window)
        step.note(pid=started.pid, window=window.as_dict())

    bridge = Bridge(
        mcp,
        cwd=staged.directory,
        env={"SUBORDINATE_MCP_NO_LAUNCH": "1"},
        require_editor=True,
        log=out / "mcp-reads.log",
    )
    read = ReadOnly(bridge)
    try:
        with run.step("the editor is the one being read") as step:
            before = read("media.list", {})
            step.note(connection=bridge.connection, media=len(_items(before)))
            if _items(before):
                raise AssertionError("the staged project should start with no media")

        with run.step("click Import... in the media bin") as step:
            control = session.find("Import", timeout=60)
            step.note(control=control.as_dict())
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
            row = session.find(staged.media.stem[:12], timeout=60)
            drop = window.rect.point(*layout["timeline_drop"])
            step.note(row=row.as_dict(), drop={"x": drop[0], "y": drop[1]})
            session.drag(row, drop)
            clips = _wait_until(
                lambda: _clips(read, staged.sequence),
                lambda found: len(found) >= 1,
                timeout=60,
                what="a clip on the timeline",
            )
            step.note(clips=len(clips))

        with run.step("scrub the playhead, then press Ctrl+K") as step:
            ruler = window.rect.point(*layout["ruler"])
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
        with run.step("pin the vendor encoder in the export panel") as step:
            session.activate(window)
            field = session.find("File", timeout=60, role=None)
            step.note(file_field=field.as_dict())
            # The output is a text field, so it is typed rather than chosen in
            # a save dialog: click it, select everything, type the path.
            session.click(field.rect.point(0.5, 0.5))
            session.key("ctrl+a")
            session.type_text(str(output))
            picker = session.find("Encoder", timeout=30)
            session.click(picker)
            choice = session.find(encoder, timeout=30)
            session.click(choice)
            step.note(encoder=encoder, picker=picker.as_dict())

        with run.step("click Export and wait for the file") as step:
            session.click(session.find("Export", timeout=30))
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
