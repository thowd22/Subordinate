"""The X11 half of the harness: xdotool for input, scrot for pictures.

Everything here assumes the Linux desktop image (infra/images/linux-desktop):
Xorg on the NVIDIA driver at `:0`, openbox, and xdotool, xwininfo and scrot on
PATH. `DISPLAY` and `XAUTHORITY` come from the environment, because a GitHub
Actions step on that image is started by the RunsOn bootstrap and reads no
login profile.

Names come from AT-SPI when the accessibility bus is up. egui publishes its
tree through AccessKit, whose Unix adapter registers on the a11y bus, so a
session with `at-spi2-core` running can ask for `Import...` by name; one
without it has no tree at all, and :meth:`LinuxSession.controls` says so rather
than returning an empty list that would read as "the button is not there".
"""

from __future__ import annotations

import os
import subprocess
import time
from pathlib import Path

from .session import (
    Control,
    DesktopError,
    Launched,
    Rect,
    Session,
    Window,
    require,
    run,
)


class LinuxSession(Session):
    """A session on an X display."""

    def __init__(self, shots: Path | str = "shots") -> None:
        super().__init__(shots)
        self.display = os.environ.get("DISPLAY", ":0")
        os.environ.setdefault("DISPLAY", self.display)
        self._xdotool = require("xdotool")
        self._registry = None

    # ------------------------------------------------------------------ launch

    def launch(
        self,
        command: list[str],
        *,
        log: Path | str | None = None,
        cwd: Path | str | None = None,
        env: dict[str, str] | None = None,
    ) -> Launched:
        handle = None
        if log is not None:
            log = Path(log)
            log.parent.mkdir(parents=True, exist_ok=True)
            handle = log.open("wb")
        environment = dict(os.environ)
        environment.update(env or {})
        process = subprocess.Popen(
            command,
            stdout=handle or subprocess.DEVNULL,
            stderr=subprocess.STDOUT if handle else subprocess.DEVNULL,
            stdin=subprocess.DEVNULL,
            cwd=str(cwd) if cwd else None,
            env=environment,
            # A session of its own, so the editor outlives the step that
            # started it and a later kill takes the whole group.
            start_new_session=True,
        )
        return Launched(pid=process.pid, log=Path(log) if log else None, popen=process)

    def stop(self, launched: Launched) -> None:
        try:
            os.killpg(os.getpgid(launched.pid), 15)
        except (ProcessLookupError, PermissionError):
            pass

    # ------------------------------------------------------------------- waits

    def wait_for_title(self, pattern: str, *, timeout: float = 120.0) -> Window:
        deadline = time.monotonic() + timeout
        while True:
            found = subprocess.run(
                [self._xdotool, "search", "--name", pattern],
                capture_output=True,
                text=True,
                check=False,
            )
            ids = [line for line in found.stdout.split() if line.strip()]
            for window_id in ids:
                geometry = self._geometry(window_id)
                if geometry is None:
                    continue
                title = subprocess.run(
                    [self._xdotool, "getwindowname", window_id],
                    capture_output=True,
                    text=True,
                    check=False,
                ).stdout.strip()
                return Window(handle=window_id, title=title, rect=geometry)
            if time.monotonic() >= deadline:
                raise DesktopError(
                    f"no window matching {pattern!r} on {self.display} within {timeout:g}s"
                )
            time.sleep(0.5)

    def _geometry(self, window_id: str) -> Rect | None:
        shell = subprocess.run(
            [self._xdotool, "getwindowgeometry", "--shell", window_id],
            capture_output=True,
            text=True,
            check=False,
        )
        if shell.returncode != 0:
            return None
        values: dict[str, int] = {}
        for line in shell.stdout.splitlines():
            key, _, value = line.partition("=")
            if value.strip().lstrip("-").isdigit():
                values[key.strip()] = int(value)
        if {"X", "Y", "WIDTH", "HEIGHT"} <= values.keys():
            return Rect(values["X"], values["Y"], values["WIDTH"], values["HEIGHT"])
        return None

    def activate(self, window: Window | str) -> None:
        """Raise a window and give it the keyboard, so input lands in it."""
        handle = window.handle if isinstance(window, Window) else window
        subprocess.run(
            [self._xdotool, "windowactivate", "--sync", handle], check=False
        )
        subprocess.run([self._xdotool, "windowraise", handle], check=False)

    # ---------------------------------------------------------------- controls

    def _at_spi(self):
        """The AT-SPI registry, or an error saying the bus is not there."""
        if self._registry is None:
            try:
                import pyatspi  # type: ignore
            except ImportError as error:  # pragma: no cover - depends on the image
                raise DesktopError(
                    "python3-pyatspi is not installed, so AccessKit names cannot be "
                    "read on this display; install at-spi2-core and python3-pyatspi, "
                    "and start the application on a session bus"
                ) from error
            self._registry = pyatspi
        return self._registry

    def controls(self, *, window: str | None = None) -> list[Control]:
        pyatspi = self._at_spi()
        found: list[Control] = []
        desktop = pyatspi.Registry.getDesktop(0)
        for application in desktop:
            if application is None:
                continue
            try:
                name = application.name or ""
            except Exception:  # noqa: BLE001 - a dead application is skipped
                continue
            if window is not None and window not in name:
                continue
            _collect(application, found, 0)
        return found

    # ------------------------------------------------------------------- input

    def click(self, target, *, button: int = 1, double: bool = False) -> tuple[int, int]:
        x, y = self.point_of(target)
        arguments = [
            self._xdotool,
            "mousemove",
            "--sync",
            str(x),
            str(y),
            "click",
        ]
        if double:
            arguments += ["--repeat", "2", "--delay", "80"]
        arguments.append(str(button))
        run(arguments)
        time.sleep(0.3)
        return (x, y)

    def maximize(self, window: Window) -> Window:
        # openbox honours _NET_WM_STATE_MAXIMIZED, and xdotool asks for it
        # through windowsize's percentage form, which is the same thing
        # without needing a window manager hint.
        subprocess.run(
            [self._xdotool, "windowmove", "--sync", window.handle, "0", "0"], check=False
        )
        subprocess.run(
            [self._xdotool, "windowsize", "--sync", window.handle, "100%", "100%"],
            check=False,
        )
        time.sleep(1.0)
        rect = self._geometry(window.handle) or window.rect
        return Window(handle=window.handle, title=window.title, rect=rect)

    def click_free_move(self, x: int, y: int) -> None:
        run([self._xdotool, "mousemove", "--sync", str(x), str(y)])
        time.sleep(0.4)

    def key(self, keys: str) -> None:
        run([self._xdotool, "key", "--clearmodifiers", keys])
        time.sleep(0.2)

    def type_text(self, text: str) -> None:
        run([self._xdotool, "type", "--clearmodifiers", "--delay", "25", text])
        time.sleep(0.2)

    def drag(self, source, destination, *, steps: int = 24, hold: float = 0.4) -> None:
        x0, y0 = self.point_of(source)
        x1, y1 = self.point_of(destination)
        run([self._xdotool, "mousemove", "--sync", str(x0), str(y0)])
        time.sleep(hold)
        run([self._xdotool, "mousedown", "1"])
        time.sleep(hold)
        for step in range(1, steps + 1):
            x = x0 + (x1 - x0) * step // steps
            y = y0 + (y1 - y0) * step // steps
            run([self._xdotool, "mousemove", "--sync", str(x), str(y)])
            time.sleep(0.04)
        time.sleep(hold)
        run([self._xdotool, "mouseup", "1"])
        time.sleep(hold)

    # -------------------------------------------------------------- screenshot

    def screenshot(self, name: str) -> Path:
        path = self.shots / name if not Path(name).is_absolute() else Path(name)
        path.parent.mkdir(parents=True, exist_ok=True)
        scrot = require("scrot")
        run([scrot, "--overwrite", str(path)], timeout=60)
        return path


def _collect(node, found: list[Control], depth: int) -> None:
    """Walk one AT-SPI subtree, keeping every node that has a name and a box."""
    if depth > 24:
        return
    try:
        children = list(node)
    except Exception:  # noqa: BLE001 - a node that went away mid-walk
        children = []
    try:
        name = node.name or ""
        role = node.getRoleName()
        component = node.queryComponent()
        import pyatspi  # type: ignore

        box = component.getExtents(pyatspi.DESKTOP_COORDS)
        if name and box.width > 0 and box.height > 0:
            found.append(
                Control(
                    name=name,
                    role=role,
                    rect=Rect(box.x, box.y, box.width, box.height),
                )
            )
    except Exception:  # noqa: BLE001 - nodes without a component are skipped
        pass
    for child in children:
        if child is not None:
            _collect(child, found, depth + 1)
