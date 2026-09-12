"""Drive the real Subordinate window on a desktop CI runner (TASK-139).

    from subdesktop import open_session

    desktop = open_session("shots")
    app = desktop.launch(["subordinate", "flow.sub"], log="app.log")
    desktop.wait_for_title("Subordinate")
    desktop.click("Import...")
    desktop.screenshot("01-import.png")

The same seven verbs - launch, click, key, drag, wait_for_title, wait_for_log,
screenshot - work on both desktop images. See README.md.
"""

from .session import (
    Control,
    DesktopError,
    Launched,
    Rect,
    Session,
    Window,
    open_session,
)

__all__ = [
    "Control",
    "DesktopError",
    "Launched",
    "Rect",
    "Session",
    "Window",
    "open_session",
]
