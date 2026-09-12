#!/usr/bin/env python3
"""Which kind of synthetic click does the editor on this X display take?

The Linux desktop runner takes pointer *motion* - the MCP flow's screenshots
prove it, because the window repaints when the pointer is nudged - but a click
aimed at a control the accessibility tree located does not reach it (runs
34677987056 and 34678850753). This tries the ways of sending one, one after
another, against a control whose effect is visible in project state: a track
header's mute button.

It is a diagnosis, not a test: it prints what each attempt did and exits 0
either way.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from subdesktop import flow as flowlib  # noqa: E402
from subdesktop.mcp import Bridge  # noqa: E402
from subdesktop.session import open_session  # noqa: E402


def xdo(*arguments: str) -> str:
    finished = subprocess.run(
        ["xdotool", *arguments], capture_output=True, text=True, check=False
    )
    return (finished.stdout + finished.stderr).strip()


def main() -> int:
    out = Path(os.environ.get("FLOW_OUT", "flow-out")).absolute()
    out.mkdir(parents=True, exist_ok=True)
    session = open_session(out / "shots")
    work = Path(os.environ.get("FLOW_WORK", str(out / "project")))
    media = Path(os.environ["FLOW_MEDIA"])
    cli = os.environ.get("FLOW_CLI", "subordinate-cli")
    editor = os.environ.get("FLOW_EDITOR", "subordinate")
    mcp = os.environ.get("FLOW_MCP", "subordinate-mcp")

    staged = flowlib.stage(work, media, cli)
    app = session.launch([editor, str(staged.project)], log=out / "editor.log")
    window = session.wait_for_title("Subordinate", timeout=180)
    window = session.maximize(window)
    flowlib.wait_for_command_api(session, out / "editor.log")
    session.activate(window)

    print("window:", json.dumps(window.as_dict()))
    print("active window:", xdo("getactivewindow"))
    print("focused window:", xdo("getwindowfocus"))
    print("pointer:", xdo("getmouselocation"))
    print("xinput:", subprocess.run(["xinput", "list"], capture_output=True, text=True).stdout[:600])

    bridge = Bridge(
        mcp,
        cwd=staged.directory,
        env={"SUBORDINATE_MCP_NO_LAUNCH": "1"},
        require_editor=True,
        log=out / "mcp.log",
    )

    def muted() -> bool:
        return '"muted": true' in json.dumps(bridge.call("project.get", {}))

    mute = session.find("M", role="push button", timeout=60)
    x, y = mute.rect.center
    print("mute button:", json.dumps(mute.as_dict()))

    attempts = {
        "xdotool click": lambda: xdo("mousemove", "--sync", str(x), str(y), "click", "1"),
        "down/up with pauses": lambda: (
            xdo("mousemove", "--sync", str(x), str(y)),
            time.sleep(0.3),
            xdo("mousedown", "1"),
            time.sleep(0.2),
            xdo("mouseup", "1"),
        ),
        "click --window": lambda: xdo("mousemove", "--sync", str(x), str(y))
        or xdo("click", "--window", window.handle, "1"),
        "click after windowactivate": lambda: xdo("windowactivate", "--sync", window.handle)
        or xdo("mousemove", "--sync", str(x), str(y), "click", "1"),
        "click at the window-relative point": lambda: xdo(
            "mousemove",
            "--sync",
            str(window.rect.x + x),
            str(window.rect.y + y),
            "click",
            "1",
        ),
    }
    for name, attempt in attempts.items():
        attempt()
        time.sleep(1.5)
        state = muted()
        session.screenshot(f"probe-{name.replace(' ', '-').replace('/', '-')}.png")
        print(f"attempt {name!r}: muted={state} pointer={xdo('getmouselocation')}")
        if state:
            print("this one reached the editor")
            break

    # Does the keyboard get through at all? The editor's own shortcut window
    # opens on a click, so this asks the simplest question instead: type into
    # the display and see whether anything in the tree changed.
    before = len(session.controls())
    xdo("key", "--clearmodifiers", "ctrl+z")
    time.sleep(1)
    print("controls before/after a keystroke:", before, len(session.controls()))

    bridge.close()
    session.stop(app)
    return 0


if __name__ == "__main__":
    sys.exit(main())
