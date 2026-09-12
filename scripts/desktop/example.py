#!/usr/bin/env python3
"""The smallest useful flow: open the editor, click something, photograph it.

    FLOW_OUT=/tmp/example python3 scripts/desktop/example.py

It works on either desktop image without changes, which is the point of the
harness: the verbs below are the same seven on both, and the button is found by
the name the source gives it rather than by where it happens to be drawn.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from subdesktop import flow as flowlib  # noqa: E402


def example(run: flowlib.Run) -> None:
    session = run.session
    editor = os.environ.get("FLOW_EDITOR", "subordinate")
    project = os.environ.get("FLOW_PROJECT", "")
    started = None

    try:
        with run.step("launch the editor") as step:
            started = session.launch(
                [editor] + ([project] if project else []), log=run.out / "editor.log"
            )
            window = session.wait_for_title("Subordinate", timeout=180)
            window = session.maximize(window)
            step.note(pid=started.pid, window=window.as_dict())

        with run.step("read the accessibility tree") as step:
            controls = session.controls()
            (run.out / "controls.json").write_text(
                json.dumps([control.as_dict() for control in controls], indent=1)
            )
            step.note(controls=sorted({control.name for control in controls})[:40])

        with run.step("click the media bin's Import button") as step:
            button = session.find("Import")
            step.note(button=button.as_dict())
            session.click(button)
            session.key("Escape")
    finally:
        # Always: an editor left running holds the Command API endpoint, and
        # the next one to start would refuse it (command.address_in_use).
        if started is not None:
            session.stop(started)


if __name__ == "__main__":
    sys.exit(flowlib.main(example, "Example"))
