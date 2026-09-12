"""The Windows half of the harness: UI Automation for names, .NET for pictures.

Everything here assumes the Windows desktop image
(infra/images/windows-desktop): the machine-wide Python 3.12 with pywinauto,
and a real console session. **It only works inside that session.** The GitHub
Actions runner is SYSTEM in session 0, which has no desktop, so a job sends
this code into the console session through
`C:\\SubordinateTest\\bin\\InteractiveSession.psm1` - see
`scripts/desktop/windows/run-flow.ps1`.

egui's AccessKit tree arrives as UI Automation, so a control is found by the
name the source gives it. Mouse input is `click_input` and real cursor moves
rather than an automation `Invoke`, because the point of these flows is that
the desktop takes the input a person's mouse would produce.
"""

from __future__ import annotations

import os
import re
import subprocess
import time
from pathlib import Path

from .session import Control, DesktopError, Launched, Rect, Session, Window

# The capture is .NET rather than a Python imaging library: the image carries
# no Pillow, and every Windows has System.Drawing.
_CAPTURE = r"""
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
$bmp.Save('{path}', [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
"""


class WindowsSession(Session):
    """A session on the interactive console desktop."""

    def __init__(self, shots: Path | str = "shots") -> None:
        super().__init__(shots)
        try:
            from pywinauto import Desktop  # type: ignore
            from pywinauto import mouse, keyboard  # type: ignore
        except ImportError as error:  # pragma: no cover - depends on the image
            raise DesktopError(
                "pywinauto is not installed; the desktop image installs it with the "
                "machine-wide Python"
            ) from error
        self._desktop = Desktop(backend="uia")
        self._mouse = mouse
        self._keyboard = keyboard
        self._window = None

    # ------------------------------------------------------------------ launch

    def launch(
        self,
        command: list[str],
        *,
        log: Path | str | None = None,
        cwd: Path | str | None = None,
        env: dict[str, str] | None = None,
    ) -> Launched:
        environment = dict(os.environ)
        environment.update(env or {})
        handle = None
        if log is not None:
            log = Path(log)
            log.parent.mkdir(parents=True, exist_ok=True)
            handle = log.open("wb")
        # DETACHED_PROCESS: the window must outlive this script, and anything
        # that inherits our handles would keep a redirection open until the
        # editor exits (the mistake run 34669759392 caught on TASK-138).
        flags = 0x00000008 | 0x00000200  # DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        process = subprocess.Popen(
            command,
            stdout=handle or subprocess.DEVNULL,
            stderr=subprocess.STDOUT if handle else subprocess.DEVNULL,
            stdin=subprocess.DEVNULL,
            cwd=str(cwd) if cwd else None,
            env=environment,
            creationflags=flags,
        )
        return Launched(pid=process.pid, log=Path(log) if log else None, popen=process)

    def stop(self, launched: Launched) -> None:
        subprocess.run(
            ["taskkill", "/PID", str(launched.pid), "/T", "/F"],
            capture_output=True,
            check=False,
        )

    # ------------------------------------------------------------------- waits

    def wait_for_title(self, pattern: str, *, timeout: float = 120.0) -> Window:
        expression = re.compile(pattern)
        deadline = time.monotonic() + timeout
        while True:
            for candidate in self._desktop.windows():
                try:
                    title = candidate.window_text() or ""
                    if not expression.search(title):
                        continue
                    box = candidate.rectangle()
                except Exception:  # noqa: BLE001 - a window that went away
                    continue
                self._window = candidate
                return Window(
                    handle=str(candidate.handle),
                    title=title,
                    rect=Rect(box.left, box.top, box.width(), box.height()),
                )
            if time.monotonic() >= deadline:
                raise DesktopError(
                    f"no window matching {pattern!r} in this session within {timeout:g}s"
                )
            time.sleep(0.5)

    # ---------------------------------------------------------------- controls

    def controls(self, *, window: str | None = None) -> list[Control]:
        roots = []
        if window is not None:
            for candidate in self._desktop.windows():
                try:
                    if window in (candidate.window_text() or ""):
                        roots.append(candidate)
                except Exception:  # noqa: BLE001
                    continue
        elif self._window is not None:
            roots = [self._window]
        else:
            roots = list(self._desktop.windows())
        found: list[Control] = []
        for root in roots:
            try:
                nodes = [root] + root.descendants()
            except Exception:  # noqa: BLE001 - a window that closed mid-walk
                continue
            for node in nodes:
                try:
                    name = node.window_text() or ""
                    role = node.element_info.control_type or ""
                    box = node.rectangle()
                except Exception:  # noqa: BLE001
                    continue
                if not name or box.width() <= 0 or box.height() <= 0:
                    continue
                found.append(
                    Control(
                        name=name,
                        role=role,
                        rect=Rect(box.left, box.top, box.width(), box.height()),
                    )
                )
        return found

    def activate(self, window: Window | str) -> None:
        """Put the keyboard focus on the window, ignoring a focus race."""
        try:
            if self._window is not None:
                self._window.set_focus()
        except Exception:  # noqa: BLE001 - focus races are not failures
            pass

    # ------------------------------------------------------------------- input

    def click(self, target, *, button: int = 1, double: bool = False) -> tuple[int, int]:
        x, y = self.point_of(target)
        name = {1: "left", 2: "middle", 3: "right"}.get(button, "left")
        if double:
            self._mouse.double_click(button=name, coords=(x, y))
        else:
            self._mouse.click(button=name, coords=(x, y))
        time.sleep(0.3)
        return (x, y)

    def maximize(self, window: Window) -> Window:
        if self._window is not None:
            try:
                self._window.maximize()
                time.sleep(1.0)
                box = self._window.rectangle()
                return Window(
                    handle=window.handle,
                    title=window.title,
                    rect=Rect(box.left, box.top, box.width(), box.height()),
                )
            except Exception:  # noqa: BLE001 - a window that refuses stays as it is
                pass
        return window

    def click_free_move(self, x: int, y: int) -> None:
        self._mouse.move(coords=(int(x), int(y)))
        time.sleep(0.05)

    def key(self, keys: str) -> None:
        self._keyboard.send_keys(_chord(keys))
        time.sleep(0.2)

    def type_text(self, text: str) -> None:
        self._keyboard.send_keys(text, with_spaces=True, pause=0.02)
        time.sleep(0.2)

    def press(self, x: int, y: int, *, button: int = 1) -> None:
        name = {1: "left", 2: "middle", 3: "right"}.get(button, "left")
        self._mouse.press(button=name, coords=(int(x), int(y)))

    def release(self, x: int, y: int, *, button: int = 1) -> None:
        name = {1: "left", 2: "middle", 3: "right"}.get(button, "left")
        self._mouse.release(button=name, coords=(int(x), int(y)))

    # -------------------------------------------------------------- screenshot

    def screenshot(self, name: str) -> Path:
        path = self.shots / name if not Path(name).is_absolute() else Path(name)
        path.parent.mkdir(parents=True, exist_ok=True)
        script = _CAPTURE.replace("{path}", str(path).replace("'", "''"))
        finished = subprocess.run(
            ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", script],
            capture_output=True,
            text=True,
            check=False,
            timeout=120,
        )
        if finished.returncode != 0 or not path.exists():
            raise DesktopError(f"the screen capture failed: {finished.stderr.strip()}")
        return path


#: How the shared chord spelling (`ctrl+k`) maps onto `send_keys`.
_MODIFIERS = {"ctrl": "^", "control": "^", "alt": "%", "shift": "+"}
_NAMED = {
    "return": "{ENTER}",
    "enter": "{ENTER}",
    "escape": "{ESC}",
    "esc": "{ESC}",
    "tab": "{TAB}",
    "space": "{SPACE}",
    "backspace": "{BACKSPACE}",
    "delete": "{DELETE}",
    "home": "{HOME}",
    "end": "{END}",
    "up": "{UP}",
    "down": "{DOWN}",
    "left": "{LEFT}",
    "right": "{RIGHT}",
}


def _chord(keys: str) -> str:
    """Turn `ctrl+k` into `^k`, and `Return` into `{ENTER}`.

    The spelling is xdotool's, because that is the one a flow script writes
    once and both backends must understand.
    """
    parts = keys.split("+")
    prefix = ""
    for part in parts[:-1]:
        modifier = _MODIFIERS.get(part.lower())
        if modifier is None:
            raise DesktopError(f"unknown modifier {part!r} in {keys!r}")
        prefix += modifier
    last = parts[-1]
    body = _NAMED.get(last.lower(), last.lower() if len(last) == 1 else f"{{{last.upper()}}}")
    return prefix + body
