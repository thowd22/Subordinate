"""Click the media bin's Import button through UI Automation (TASK-138).

Runs with the machine-wide Python on the image, inside the auto-logon user's
console session. egui publishes its widget tree through AccessKit, which
Windows exposes as UI Automation, so the button can be found by the name the
source gives it ("Import...", crates/sub-ui/src/media_bin.rs) instead of by
pixel guessing.

Exit codes, so the workflow can say what went wrong:
  0  clicked, and the native file dialog appeared
  2  no editor window
  3  no Import button in the editor's UI Automation tree
  4  clicked, but no file dialog appeared
"""

import sys
import time

from pywinauto import Desktop


def find_editor(timeout=60):
    deadline = time.time() + timeout
    while time.time() < deadline:
        for w in Desktop(backend="uia").windows():
            try:
                if "Subordinate" in (w.window_text() or ""):
                    return w
            except Exception:
                continue
        time.sleep(1)
    return None


def find_import_button(win, timeout=30):
    deadline = time.time() + timeout
    names = []
    while time.time() < deadline:
        names = []
        for c in win.descendants(control_type="Button"):
            try:
                name = c.window_text() or ""
            except Exception:
                continue
            names.append(name)
            if name.startswith("Import"):
                return c, names
        time.sleep(1)
    return None, names


def find_dialog(editor_handle, timeout=20):
    deadline = time.time() + timeout
    while time.time() < deadline:
        for w in Desktop(backend="uia").windows():
            try:
                if w.handle == editor_handle:
                    continue
                title = w.window_text() or ""
                cls = w.class_name() or ""
            except Exception:
                continue
            # The GTK-style portal is not a thing on Windows: rfd opens the
            # common item dialog, whose window class is #32770.
            if cls == "#32770" or "Open" in title or "Import" in title:
                return w
        time.sleep(1)
    return None


def main():
    win = find_editor()
    if win is None:
        print("NO WINDOW")
        return 2
    print("window:", repr(win.window_text()), win.rectangle())
    try:
        win.set_focus()
    except Exception as error:  # a focus race is not fatal
        print("set_focus:", error)

    button, names = find_import_button(win)
    if button is None:
        print("buttons seen:", names[:60])
        print("NO IMPORT BUTTON")
        return 3
    print("button:", repr(button.window_text()), button.rectangle())

    # A real mouse move and click at the button's screen rectangle, not an
    # automation Invoke: the point of this job is to prove the desktop takes
    # input the way a person's would.
    button.click_input()
    time.sleep(2)

    dialog = find_dialog(win.handle)
    if dialog is None:
        print("NO DIALOG")
        return 4
    print("dialog:", repr(dialog.window_text()), dialog.class_name(), dialog.rectangle())
    return 0


if __name__ == "__main__":
    sys.exit(main())
