"""The harness as a command, so a shell or PowerShell step drives it too.

    python3 -m subdesktop launch --log app.log -- subordinate flow.sub
    python3 -m subdesktop wait-for-title Subordinate
    python3 -m subdesktop click --name 'Import...'
    python3 -m subdesktop key ctrl+k
    python3 -m subdesktop drag --from-name 'meld' --to 900,700
    python3 -m subdesktop screenshot 02-after-import.png
    python3 -m subdesktop controls

Every subcommand prints JSON, so a step can assert on what it did.
"""

from __future__ import annotations

import argparse
import json
import sys

from .session import DesktopError, open_session


def _target(session, name: str | None, point: str | None):
    if name:
        return session.find(name)
    if point:
        x, _, y = point.partition(",")
        return (int(x), int(y))
    raise DesktopError("give either --name or a point")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="subdesktop", description=__doc__)
    parser.add_argument("--shots", default="shots", help="where screenshots go")
    sub = parser.add_subparsers(dest="command", required=True)

    launch = sub.add_parser("launch", help="start an application, detached")
    launch.add_argument("--log", default=None)
    launch.add_argument("--cwd", default=None)
    launch.add_argument("argv", nargs=argparse.REMAINDER)

    title = sub.add_parser("wait-for-title", help="wait for a window title")
    title.add_argument("pattern")
    title.add_argument("--timeout", type=float, default=120.0)

    log = sub.add_parser("wait-for-log", help="wait for a line in a log file")
    log.add_argument("path")
    log.add_argument("pattern")
    log.add_argument("--timeout", type=float, default=120.0)

    click = sub.add_parser("click", help="click a named control or a point")
    click.add_argument("--name")
    click.add_argument("--at", dest="point")
    click.add_argument("--button", type=int, default=1)
    click.add_argument("--double", action="store_true")

    key = sub.add_parser("key", help="send one chord, such as ctrl+k")
    key.add_argument("keys")

    typing = sub.add_parser("type", help="type text")
    typing.add_argument("text")

    drag = sub.add_parser("drag", help="drag from one target to another")
    drag.add_argument("--from-name", dest="from_name")
    drag.add_argument("--from", dest="from_point")
    drag.add_argument("--to-name", dest="to_name")
    drag.add_argument("--to", dest="to_point")
    drag.add_argument("--steps", type=int, default=24)

    shot = sub.add_parser("screenshot", help="photograph the whole desktop")
    shot.add_argument("name")

    controls = sub.add_parser("controls", help="dump the accessibility tree")
    controls.add_argument("--window", default=None)
    controls.add_argument("--name", default=None, help="only names starting with this")

    options = parser.parse_args(argv)
    session = open_session(options.shots)

    if options.command == "launch":
        argv_rest = [item for item in options.argv if item != "--"]
        started = session.launch(argv_rest, log=options.log, cwd=options.cwd)
        print(json.dumps({"pid": started.pid, "log": str(started.log or "")}))
    elif options.command == "wait-for-title":
        window = session.wait_for_title(options.pattern, timeout=options.timeout)
        print(json.dumps(window.as_dict()))
    elif options.command == "wait-for-log":
        line = session.wait_for_log(options.path, options.pattern, timeout=options.timeout)
        print(json.dumps({"line": line}))
    elif options.command == "click":
        point = session.click(
            _target(session, options.name, options.point),
            button=options.button,
            double=options.double,
        )
        print(json.dumps({"clicked": {"x": point[0], "y": point[1]}}))
    elif options.command == "key":
        session.key(options.keys)
        print(json.dumps({"keys": options.keys}))
    elif options.command == "type":
        session.type_text(options.text)
        print(json.dumps({"typed": options.text}))
    elif options.command == "drag":
        session.drag(
            _target(session, options.from_name, options.from_point),
            _target(session, options.to_name, options.to_point),
            steps=options.steps,
        )
        print(json.dumps({"dragged": True}))
    elif options.command == "screenshot":
        path = session.screenshot(options.name)
        print(json.dumps({"screenshot": str(path)}))
    elif options.command == "controls":
        found = session.controls(window=options.window)
        if options.name:
            found = [c for c in found if c.name.startswith(options.name)]
        print(json.dumps([control.as_dict() for control in found], indent=1))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except DesktopError as error:
        print(f"subdesktop: {error}", file=sys.stderr)
        sys.exit(3)
