"""The command set a desktop flow drives the real application with.

One vocabulary, two implementations: :mod:`subdesktop.linux` speaks X11
(xdotool, scrot, xwininfo, and AT-SPI for names) and :mod:`subdesktop.windows`
speaks UI Automation (pywinauto, and .NET for the screen capture). Everything a
flow script does goes through the verbs below, so the same flow file runs on
both desktop images (TASK-139).

Controls are addressed **by name** wherever the toolkit publishes one. egui
publishes its widget tree through AccessKit, which Windows exposes as UI
Automation and Linux exposes over AT-SPI, so `Import...` is the button the
source calls `Import...` (crates/sub-ui/src/media_bin.rs) rather than a pixel
guess that breaks the first time a panel moves. A point is still accepted,
because a timeline canvas is one widget with no named children and a drop
inside it has to land somewhere in particular.
"""

from __future__ import annotations

import abc
import os
import re
import shutil
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path


class DesktopError(RuntimeError):
    """Anything the harness could not do: no window, no control, no display."""


@dataclass(frozen=True)
class Rect:
    """A screen rectangle in pixels, origin top-left."""

    x: int
    y: int
    width: int
    height: int

    @property
    def center(self) -> tuple[int, int]:
        return (self.x + self.width // 2, self.y + self.height // 2)

    def point(self, fx: float, fy: float) -> tuple[int, int]:
        """A point inside, as fractions of the width and height."""
        return (self.x + int(self.width * fx), self.y + int(self.height * fy))

    def as_dict(self) -> dict:
        return {"x": self.x, "y": self.y, "width": self.width, "height": self.height}


@dataclass(frozen=True)
class Control:
    """One node of the application's accessibility tree."""

    name: str
    role: str
    rect: Rect

    def as_dict(self) -> dict:
        return {"name": self.name, "role": self.role, "rect": self.rect.as_dict()}


@dataclass(frozen=True)
class Window:
    """A top-level window: whatever the backend identifies it by, and where it is."""

    handle: str
    title: str
    rect: Rect

    def as_dict(self) -> dict:
        return {"handle": self.handle, "title": self.title, "rect": self.rect.as_dict()}


@dataclass
class Launched:
    """A process this harness started."""

    pid: int
    log: Path | None = None
    popen: subprocess.Popen | None = field(default=None, repr=False)


# A target is a control name, a Control, a Window, or an absolute point.
Target = "str | Control | Window | tuple[int, int]"


class Session(abc.ABC):
    """The desktop of one machine, as a flow script sees it."""

    #: Where screenshots go when a relative name is given.
    shots: Path

    def __init__(self, shots: Path | str = "shots") -> None:
        self.shots = Path(shots)
        self.shots.mkdir(parents=True, exist_ok=True)
        self._shot_index = 0

    # ------------------------------------------------------------------ launch

    @abc.abstractmethod
    def launch(
        self,
        command: list[str],
        *,
        log: Path | str | None = None,
        cwd: Path | str | None = None,
        env: dict[str, str] | None = None,
    ) -> Launched:
        """Start `command` detached, with its output going to `log`.

        Detached on purpose: the window has to outlive the shell that started
        it, because every later verb runs in a process of its own.
        """

    @abc.abstractmethod
    def stop(self, launched: Launched) -> None:
        """Close the application, ignoring one that has already gone."""

    # ------------------------------------------------------------------- waits

    @abc.abstractmethod
    def wait_for_title(self, pattern: str, *, timeout: float = 120.0) -> Window:
        """Wait for a top-level window whose title matches `pattern`."""

    def wait_for_log(
        self, log: Path | str, pattern: str, *, timeout: float = 120.0
    ) -> str:
        """Wait for a line matching `pattern` in `log`, and return it.

        Shared by both backends: a log file is a log file. The whole file is
        re-read each time rather than followed, because the application may
        rewrite or buffer it and a few kilobytes a second costs nothing.
        """
        expression = re.compile(pattern)
        path = Path(log)
        deadline = time.monotonic() + timeout
        while True:
            if path.exists():
                text = path.read_text(encoding="utf-8", errors="replace")
                for line in text.splitlines():
                    if expression.search(line):
                        return line
            if time.monotonic() >= deadline:
                tail = ""
                if path.exists():
                    tail = "\n".join(
                        path.read_text(encoding="utf-8", errors="replace").splitlines()[-20:]
                    )
                raise DesktopError(
                    f"{pattern!r} never appeared in {path} within {timeout:g}s\n{tail}"
                )
            time.sleep(0.5)

    # ---------------------------------------------------------------- controls

    @abc.abstractmethod
    def controls(self, *, window: str | None = None) -> list[Control]:
        """Every named control of the application's accessibility tree."""

    def find(
        self,
        name: str,
        *,
        role: str | None = None,
        timeout: float = 20.0,
        window: str | None = None,
    ) -> Control:
        """The first control whose name starts with `name`, waited for.

        Prefix matching, because egui labels carry decoration a test should not
        have to spell: the media bin's button is `Import...` with an ellipsis
        that is three dots in one build and one character in another.
        """
        deadline = time.monotonic() + timeout
        seen: list[str] = []
        while True:
            seen = []
            for control in self.controls(window=window):
                seen.append(f"{control.role}:{control.name}")
                if role is not None and control.role != role:
                    continue
                if control.name == name or control.name.startswith(name):
                    return control
            if time.monotonic() >= deadline:
                raise DesktopError(
                    f"no control named {name!r}"
                    + (f" with role {role!r}" if role else "")
                    + f" within {timeout:g}s; the tree held: {sorted(set(seen))[:80]}"
                )
            time.sleep(0.5)

    def point_of(self, target) -> tuple[int, int]:
        """The screen point a target names."""
        if isinstance(target, tuple):
            return (int(target[0]), int(target[1]))
        if isinstance(target, (Control, Window)):
            return target.rect.center
        if isinstance(target, str):
            return self.find(target).rect.center
        raise DesktopError(f"not a target: {target!r}")

    # ------------------------------------------------------------------- input

    @abc.abstractmethod
    def click(self, target, *, button: int = 1, double: bool = False) -> tuple[int, int]:
        """Move the real pointer to `target` and click it."""

    @abc.abstractmethod
    def key(self, keys: str) -> None:
        """Send one chord, for example `ctrl+k`, `Return` or `Escape`."""

    @abc.abstractmethod
    def type_text(self, text: str) -> None:
        """Type `text` as keystrokes, as a person would."""

    @abc.abstractmethod
    def drag(self, source, destination, *, steps: int = 24, hold: float = 0.4) -> None:
        """Press at `source`, move to `destination` in `steps`, release.

        Stepped rather than teleported: egui starts a drag only once the
        pointer has moved while a button is down, so a single jump from one
        point to another is a click, not a drag.
        """

    # -------------------------------------------------------------- screenshot

    def nudge(self, window: "Window") -> None:
        """Move the pointer inside `window`, changing nothing, to wake it.

        egui only paints when something asks it to. An edit that arrives over
        the Command API socket lands in the project the panels draw, but on an
        idle desktop - no pointer, no keyboard, no animation - the window can
        keep showing the frame it painted before, so a screenshot taken
        straight after an agent's edit photographs a stale picture of a project
        that has already changed (run 34672010010). A pointer move is the
        cheapest thing that wakes the event loop and mutates nothing.
        """
        self.click_free_move(*window.rect.point(0.5, 0.45))

    @abc.abstractmethod
    def click_free_move(self, x: int, y: int) -> None:
        """Move the pointer to a point without pressing anything."""

    @abc.abstractmethod
    def maximize(self, window: "Window") -> "Window":
        """Fill the screen with `window`, and answer its new rectangle.

        Worth doing before any gesture: a window larger than the screen hides
        the panels at its bottom edge behind the taskbar, and a drop aimed at
        the timeline lands on the desktop instead (run 34672165182).
        """

    @abc.abstractmethod
    def screenshot(self, name: str) -> Path:
        """Capture the whole desktop into `self.shots`, and return the file."""

    def numbered_screenshot(self, label: str) -> Path:
        """A screenshot named `NN-label.png`, counting up from the first."""
        self._shot_index += 1
        safe = re.sub(r"[^a-z0-9]+", "-", label.lower()).strip("-")
        return self.screenshot(f"{self._shot_index:02d}-{safe}.png")


def open_session(shots: Path | str = "shots") -> Session:
    """The session for this machine: X11 on Linux, UI Automation on Windows."""
    if os.name == "nt":
        from .windows import WindowsSession

        return WindowsSession(shots)
    from .linux import LinuxSession

    return LinuxSession(shots)


def require(tool: str) -> str:
    """The absolute path of `tool`, or a clear error naming what is missing."""
    found = shutil.which(tool)
    if not found:
        raise DesktopError(f"{tool} is not on PATH; the desktop image should carry it")
    return found


def run(command: list[str], *, timeout: float = 30.0) -> str:
    """Run a helper and return its stdout, with its stderr in any error."""
    finished = subprocess.run(
        command, capture_output=True, text=True, timeout=timeout, check=False
    )
    if finished.returncode != 0:
        raise DesktopError(
            f"{' '.join(command)} exited {finished.returncode}: "
            f"{finished.stderr.strip() or finished.stdout.strip()}"
        )
    return finished.stdout
