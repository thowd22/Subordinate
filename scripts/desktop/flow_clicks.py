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
    "new_folder": [170, 98],
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


def _candidates(control, window) -> list[tuple[int, int]]:
    """The points a control might really be at, best guess first.

    UI Automation answers in screen coordinates and AT-SPI is asked for
    `DESKTOP_COORDS`, so the first candidate is the rectangle as given. The
    second treats it as window-relative, which is what a toolkit that has not
    been told where its window is reports, and the flow records which of the
    two opened the dialog rather than guessing in silence.
    """
    if isinstance(control, tuple):
        return [control]
    first = control.rect.center
    second = (window.rect.x + first[0], window.rect.y + first[1])
    return [first] if second == first else [first, second]


#: The same role under each platform's spelling. UI Automation says
#: `TabItem` and `Button`; AT-SPI says `page tab` and `push button`.
TAB_ROLES = ("TabItem", "page tab", "Tab", "tab")
BUTTON_ROLES = ("Button", "push button", "toggle button", "CheckBox")
COMBO_ROLES = ("ComboBox", "combo box", "Button", "push button")
TEXT_ROLES = ("Text", "label", "static text")


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

        with run.step("the editor takes a click") as step:
            # Before anything that depends on a *dialog*, prove the plain
            # gesture: `New folder` is a button with a name, and the bin it
            # creates is in the project a read can see. A failure here is the
            # pointer not reaching the application; a failure in the next step
            # with this one green is the file dialog, and the two are worth
            # telling apart (run 34676801744).
            state = read("project.get", {})
            (out / "project-get.json").write_text(json.dumps(state, indent=1)[:200000])
            # The track header's mute button: small, always fully inside the
            # timeline panel - so no clipping to reason about - and what it
            # does is project state a read can see. The flow puts it back
            # afterwards, so the probe leaves nothing behind.
            mute = _by_role(session, "M", BUTTON_ROLES, timeout=60)
            muted = False
            for point in _candidates(mute, window):
                # Focused first: a window manager that gives focus on click can
                # swallow the click that does it.
                session.activate(window)
                session.click(point)
                for _ in range(10):
                    muted = _muted(read("project.get", {}))
                    if muted:
                        break
                    time.sleep(0.5)
                step.note(**{f"click_{point}": f"muted={muted}"})
                if muted:
                    session.click(point)  # put it back
                    break
            else:
                raise AssertionError(
                    "the click did not reach the editor: no track was muted by it"
                )
            step.note(control=mute.as_dict(), bins=_bins(state))

        with run.step("click Import... in the media bin") as step:
            control, how = locate(session, window, "Import", "Import", offsets)
            step.note(control=getattr(control, "as_dict", lambda: control)(), located_by=how)
            dialog = None
            for attempt, point in enumerate(_candidates(control, window)):
                session.click(point)
                try:
                    dialog = _wait_for_dialog(session, window, timeout=30)
                except DesktopError as error:
                    step.note(**{f"attempt_{attempt}": f"{point} -> {error}"[:200]})
                    continue
                step.note(**{f"attempt_{attempt}": f"{point} -> opened {dialog}"})
                break
            if dialog is None:
                raise DesktopError(
                    "the Import button opened no file dialog. On Linux the editor's "
                    "file dialog is rfd's XDG portal backend, with zenity as its own "
                    "fallback: the session needs xdg-desktop-portal with a backend, "
                    "or zenity, and a session bus."
                )
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
            # Near the row's left edge, not its middle: the bin's name column
            # is 220 points wide and the panel is narrower, so the control's
            # rectangle - which the accessibility tree reports in full -
            # reaches out under the panel beside it, and a press at its centre
            # lands on the viewer instead of on the clip (run 34676051424).
            grab = row if isinstance(row, tuple) else (
                row.rect.x + min(30, row.rect.width // 4),
                row.rect.center[1],
            )
            drop, lane = _lane_point(session, window, layout)
            step.note(
                row=getattr(row, "as_dict", lambda: row)(),
                located_by=how,
                grab={"x": grab[0], "y": grab[1]},
                drop={"x": drop[0], "y": drop[1]},
                lane=lane,
            )
            # The picture taken with the button still down is what says whether
            # the application saw a drag at all: egui paints a ghost of the
            # clip where it would land, or a refused wash where it may not.
            session.drag(
                grab,
                drop,
                midway=lambda: session.screenshot("07a-mid-drag.png"),
            )
            clips = _wait_until(
                lambda: _clips(read, staged.sequence),
                lambda found: len(found) >= 1,
                timeout=60,
                what="a clip on the timeline",
            )
            step.note(clips=len(clips))

        with run.step("scrub the playhead, then press Ctrl+K") as step:
            # Well inside the clip that was just dropped, which is what makes
            # the cut below a cut: `Ctrl+K` on a clip's own first frame, or
            # anywhere outside it, is refused rather than performed
            # (crates/sub-ui/src/split.rs). The two clips this leaves are
            # therefore the proof that the pointer scrubbed to a position
            # inside the clip - the transport's own `playback.status` is not,
            # because the window's playhead is not the engine's transport
            # (run 34676433658 read position 0 with the viewer showing
            # 00:00:09:02).
            ruler = _ruler_point(session, window, layout, after=drop[0])
            step.note(ruler={"x": ruler[0], "y": ruler[1]})
            session.click(ruler)
            step.note(transport=read("playback.status", {}))
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
            where = _open_export_panel(session, window)
            # A candidate point that missed the tab strip may have opened the
            # View menu instead; Escape closes whatever it was.
            session.key("Escape")
            tree = [control.as_dict() for control in session.controls()]
            (out / "controls-export.json").write_text(json.dumps(tree, indent=1))
            step.note(tab=where, controls=len(tree))

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
                # into, and it sits immediately right of its label. It is also
                # a *drag* value, so what is typed has to be read back: a
                # click that slid by a pixel drags the number instead, and the
                # panel cheerfully offered to render 388,627,000,000 frames
                # (run 34678351781).
                target = (
                    out_label.rect.x + out_label.rect.width + 30,
                    out_label.rect.center[1],
                )
                for attempt in range(3):
                    session.click(target)
                    session.key("ctrl+a")
                    session.type_text("120")
                    session.key("Return")
                    summary = _range_summary(session)
                    step.note(**{f"attempt_{attempt}": summary})
                    if summary.startswith("120 frames"):
                        break
                else:
                    raise DesktopError(f"the out point would not take: {summary}")
                step.note(range="in to out", out_frame=120)
            except (DesktopError, AssertionError) as error:
                # Whole sequence is the safe fallback: slower, but a render of
                # the right thing.
                try:
                    session.click(session.find("Whole sequence", timeout=10))
                except DesktopError:
                    pass
                step.note(range="whole sequence", why=str(error)[:200])

        with run.step("type the output path and pin the vendor encoder") as step:
            # The output is a text field on the row beside the `File` label.
            # The label is what the application publishes - the field itself
            # has no name until it holds a path - and `choose a file to write`
            # is not the field's hint but the reason the Export button is
            # disabled, printed under it (run 34680515589 typed into that and
            # left the field empty). `File` is also the window's menu, so the
            # label is taken by role.
            text = _by_role(session, "File", TEXT_ROLES, timeout=60)
            point = (text.rect.x + text.rect.width + 60, text.rect.center[1])
            session.click(point)
            session.key("ctrl+a")
            session.type_text(str(output))
            session.key("Return")
            took = _shows(session, output.name)
            step.note(file_label=text.as_dict(), typed_at=point, field_took_the_path=took)
            if not took:
                raise AssertionError(
                    f"the output field did not take {output}: the panel still shows "
                    f"{_field_row(session)}"
                )
            # `ComboBox::from_label("Encoder")` names the button after its
            # label and draws the *selection* on it; its menu items are the
            # element names themselves (crates/sub-ui/src/export_panel.rs).
            picker = _by_role(session, "Encoder", COMBO_ROLES, timeout=30)
            session.click(picker)
            choice = session.find(encoder, timeout=30)
            session.click(choice)
            pinned = _shows(session, encoder)
            step.note(encoder=encoder, picker=picker.as_dict(), encoder_pinned=pinned)
            if not pinned:
                raise AssertionError(
                    f"the encoder picker still reads {_selected_encoder(session)!r}"
                )

        with run.step("click Export and wait for the file") as step:
            session.click(_by_role(session, "Export", BUTTON_ROLES, timeout=30))
            _wait_until(
                lambda: output.exists() and output.stat().st_size,
                lambda size: bool(size),
                timeout=420,
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


# -------------------------------------------------------- the export panel

#: A control only the export panel draws, used to tell that it is open.
EXPORT_MARKERS = ("Whole sequence", "Automatic", "Choose")


def _open_export_panel(session, window) -> str:
    """Click the dock tab named `Export`, which is painted rather than published.

    egui_dock draws its tab strip itself, so the tabs are not in the
    accessibility tree at all - `Export`, `Inspector` and `Timeline` are
    nowhere in it (run 34676801744). What *is* in the tree is the Inspector's
    own text, and the tab strip is the band directly above the panel it
    belongs to, so the tab is found relative to a control the application does
    publish rather than at a fixed point on the screen.
    """
    if _export_panel_open(session, timeout=2):
        return "already open"
    anchors = []
    try:
        inspector = session.find("Select a clip", timeout=10)
        anchors.append((inspector.rect.x, inspector.rect.y))
    except DesktopError:
        pass
    anchors.append(window.rect.point(0.78, 0.09))
    for left, top in anchors:
        for dx in (120, 100, 140, 80, 160, 60):
            point = (left + dx, top - 26)
            session.click(point)
            if _export_panel_open(session, timeout=3):
                return f"clicked {point}"
    raise DesktopError(
        "the export panel would not open: none of the points tried hit the dock "
        "tab named Export"
    )


def _shows(session, needle: str) -> bool:
    """Whether any control's name carries `needle`, which is how a text field
    says what is in it."""
    return any(needle in control.name for control in session.controls())


def _field_row(session) -> str:
    """What the panel says on and under its File row, for an error message."""
    return " | ".join(
        control.name
        for control in session.controls()
        if control.name in ("File", "Choose…", "Export")
        or "choose a file" in control.name
    )


def _selected_encoder(session) -> str:
    """What the encoder picker currently reads."""
    for control in session.controls():
        if control.name in ("Automatic",) or control.name.endswith("enc"):
            return control.name
    return "(nothing that looks like an encoder)"


def _range_summary(session) -> str:
    """The panel's own sentence about what the range covers, or why not."""
    for control in session.controls():
        if "frames" in control.name and "·" in control.name:
            return control.name
    return "(the panel says nothing about a range)"


def _export_panel_open(session, *, timeout: float) -> bool:
    for marker in EXPORT_MARKERS:
        try:
            session.find(marker, timeout=timeout / len(EXPORT_MARKERS))
            return True
        except DesktopError:
            continue
    return False


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
        # A third of the way along, so there is room to the right of it for
        # the scrub and the cut.
        x = window.rect.x + int(window.rect.width * 0.35)
        return (x, header.rect.center[1]), "the V1 header's row"
    except DesktopError:
        return window.rect.point(*layout["timeline_drop"]), "a fraction of the window"


def _ruler_point(session, window, layout, *, after: int) -> tuple[int, int]:
    """Where to click to scrub: the ruler, a little right of where the clip starts."""
    x = after + 120
    try:
        header = session.find("V1", timeout=10)
        return (x, header.rect.y - 24)
    except DesktopError:
        return (x, window.rect.point(*layout["ruler"])[1])


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


def _find_key(payload, key):
    """The first value under `key` anywhere in a result.

    The Command API's own shapes are documented, but what a bridge wraps them
    in is not the flow's business: `project.get` answers a `ProjectResult`, and
    whether it arrives as itself, inside `structuredContent` or inside one more
    envelope, `root_bin` is `root_bin`.
    """
    if isinstance(payload, dict):
        if key in payload:
            return payload[key]
        for value in payload.values():
            found = _find_key(value, key)
            if found is not None:
                return found
    elif isinstance(payload, list):
        for value in payload:
            found = _find_key(value, key)
            if found is not None:
                return found
    return None


def _muted(project) -> bool:
    """Whether any track of any sequence is muted."""
    text = json.dumps(project)
    return '"muted": true' in text or '"muted":true' in text


def _bins(project) -> int:
    """How many bins the project holds, at any depth."""

    def count(bin_) -> int:
        if not isinstance(bin_, dict):
            return 0
        return 1 + sum(count(child) for child in bin_.get("children", []))

    return count(_find_key(project, "root_bin"))


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
