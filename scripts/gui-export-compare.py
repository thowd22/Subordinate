#!/usr/bin/env python3
"""GUI-path export against CLI export, on the same machine and project.

TASK-143, AC 3. Every export measurement so far has gone through
`subordinate-cli render`. The editor exports through a different door -
`ExportRunner` in `sub-ui`, driven by the export panel or by `export.render` on
the Command API - and TASK-135 connected that door to the compositor readback
and the offline mix without anyone ever proving the two doors write the same
file on real hardware. This does that: one project, one preset, exported once
by the window on the Linux desktop image and once by the CLI, and the two files
compared on the thing an export is for - how many frames came back out, and
whether the sound is there.

The editor is asked through its own Command API socket (the same endpoint
`subordinate-mcp` finds through the lock file), so what is measured is the
window's export runner, not a second headless engine.

Which encoder the GUI uses is not a parameter the Command API takes, so it is
chosen the way GStreamer chooses it: `GST_PLUGIN_FEATURE_RANK`. Hardware
encoders ship ranked NONE, which is exactly why the automatic order lands on
`x264enc` by default; ranking `nvh264enc` primary puts NVENC at the front of
the same order. The CLI side pins the same element with `--encoder`, and the
export report says which element really ran, so a rank that did not take is
visible rather than assumed.
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))

import importlib.util

_spec = importlib.util.spec_from_file_location(
    "export_matrix", Path(__file__).resolve().parent / "export-matrix.py"
)
assert _spec and _spec.loader
export_matrix = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(export_matrix)


class CommandApi:
    """A JSON-RPC 2.0 client for the Command API's local socket.

    The framing is one JSON message per line, one response line per request
    (`sub_command::transport`), which is why this needs nothing but a socket
    and `json`.
    """

    def __init__(self, lock: Path, timeout: float = 1800.0) -> None:
        document = json.loads(lock.read_text(encoding="utf-8"))
        self.transport = document["transport"]
        self.address = document["address"]
        self.pid = document.get("pid")
        self._next = 0
        if self.transport == "unix-socket":
            self._socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self._socket.settimeout(timeout)
            self._socket.connect(self.address)
            self._file = self._socket.makefile("rwb")
        elif self.transport == "windows-named-pipe":
            self._socket = None
            self._file = open(self.address, "r+b", buffering=0)
        else:
            raise SystemExit(f"unknown transport {self.transport!r} in {lock}")

    def call(self, method: str, params: dict[str, Any] | None = None) -> Any:
        self._next += 1
        message = {"jsonrpc": "2.0", "method": method, "id": self._next}
        if params is not None:
            message["params"] = params
        self._file.write((json.dumps(message) + "\n").encode("utf-8"))
        self._file.flush()
        line = self._file.readline()
        if not line:
            raise SystemExit(f"the editor closed the connection during {method}")
        answer = json.loads(line.decode("utf-8"))
        if "error" in answer and answer["error"] is not None:
            raise RuntimeError(f"{method}: {json.dumps(answer['error'])}")
        return answer.get("result")

    def close(self) -> None:
        try:
            self._file.close()
        finally:
            if self._socket is not None:
                self._socket.close()


def wait_for_lock(directory: Path, instance: str, seconds: int) -> Path:
    """Waits for the editor to publish its endpoint, then hands back the file."""
    lock = directory / f"{instance}.lock.json"
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if lock.exists() and lock.stat().st_size:
            try:
                json.loads(lock.read_text(encoding="utf-8"))
                return lock
            except json.JSONDecodeError:
                pass
        time.sleep(0.5)
    raise SystemExit(f"no endpoint lock file at {lock} after {seconds}s")


def gui_export(
    api: CommandApi,
    preset: str,
    output: Path,
    frames: int | None,
    poll_seconds: int,
) -> dict[str, Any]:
    """Runs one export in the editor and waits for it to finish."""
    sequences = api.call("project.list_sequences")
    listed = sequences["sequences"]
    if not listed:
        raise SystemExit("the editor's project has no sequences to export")
    params: dict[str, Any] = {"preset": preset, "output": str(output)}
    params["sequence"] = listed[0]["id"]
    if frames:
        params["range"] = {"start_frame": 0, "end_frame": frames}
    status = api.call("export.render", params)
    job = status["job"]
    deadline = time.monotonic() + poll_seconds
    while time.monotonic() < deadline:
        status = api.call("export.progress", {"job": job})
        if status["state"] != "running":
            return status
        time.sleep(1.0)
    raise SystemExit(f"the GUI export did not finish within {poll_seconds}s")


def compare(args: argparse.Namespace) -> int:
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    prefix = args.gst_prefix or None
    endpoint_dir = Path(os.environ.get("SUBORDINATE_ENDPOINT_DIR", args.endpoint_dir))

    lock = wait_for_lock(endpoint_dir, args.instance, args.wait)
    api = CommandApi(lock)
    results: list[dict[str, Any]] = []
    failures: list[str] = []
    try:
        for encoder in args.encoders:
            gui_file = out / f"gui-{args.preset}-{encoder}.{args.container}"
            cli_file = out / f"cli-{args.preset}-{encoder}.{args.container}"

            gui_status = gui_export(api, args.preset, gui_file, args.frames, args.poll)
            cli = export_matrix.run(
                [
                    args.cli,
                    "render",
                    args.project,
                    "--preset",
                    args.preset,
                    "--encoder",
                    encoder,
                    "--out",
                    str(cli_file),
                    "--verify",
                    "--compact",
                ]
                + (["--sequence", args.sequence] if args.sequence else [])
                + (["--range", f"0:{args.frames}"] if args.frames else []),
                timeout=args.poll,
            )
            cli_report: dict[str, Any] = {}
            if cli.out.strip():
                try:
                    cli_report = json.loads(cli.out)
                except json.JSONDecodeError:
                    cli_report = {}

            row: dict[str, Any] = {
                "encoder": encoder,
                "preset": args.preset,
                "gui_state": gui_status.get("state"),
                "gui_error": gui_status.get("error"),
                "cli_ok": cli.ok,
                "cli_encoder": cli_report.get("video_encoder"),
                "cli_error": None if cli.ok else export_matrix.one_line(cli.err or cli.out),
            }
            for side, path in (("gui", gui_file), ("cli", cli_file)):
                if not path.exists():
                    row[f"{side}_frames"] = None
                    row[f"{side}_audio"] = None
                    row[f"{side}_size"] = None
                    continue
                info = export_matrix.probe(path, prefix)
                counted, how = export_matrix.count_frames(path, prefix)
                row[f"{side}_frames"] = counted
                row[f"{side}_frames_how"] = how
                row[f"{side}_audio"] = (
                    f"{info.audio[0].codec} {info.audio[0].channels}ch" if info.audio else None
                )
                row[f"{side}_video"] = info.video[0].codec if info.video else None
                row[f"{side}_size"] = path.stat().st_size
                (out / f"{side}-{encoder}-discoverer.txt").write_text(info.text, encoding="utf-8")

            if gui_status.get("state") != "completed":
                failures.append(f"{encoder}: the GUI export ended {gui_status.get('state')}")
            elif not cli.ok:
                failures.append(f"{encoder}: the CLI export failed: {row['cli_error']}")
            else:
                if row.get("gui_frames") is None or row.get("cli_frames") is None:
                    failures.append(f"{encoder}: a side could not be frame-counted")
                elif row["gui_frames"] != row["cli_frames"]:
                    failures.append(
                        f"{encoder}: the GUI wrote {row['gui_frames']} frames and the CLI"
                        f" {row['cli_frames']}"
                    )
                if bool(row.get("gui_audio")) != bool(row.get("cli_audio")):
                    failures.append(
                        f"{encoder}: audio is in the {'GUI' if row.get('gui_audio') else 'CLI'}"
                        " file only"
                    )
            row["match"] = not any(encoder in failure for failure in failures)
            results.append(row)
            print(json.dumps(row, indent=2), flush=True)
    finally:
        api.close()

    (out / "gui-vs-cli.json").write_text(
        json.dumps({"results": results, "failures": failures}, indent=2), encoding="utf-8"
    )
    lines = ["## GUI-path export against the CLI export", ""]
    lines.append("| encoder | preset | GUI frames | CLI frames | GUI audio | CLI audio | match |")
    lines.append("| --- | --- | --- | --- | --- | --- | --- |")
    for row in results:
        lines.append(
            f"| `{row['encoder']}` | {row['preset']} | {row.get('gui_frames')} |"
            f" {row.get('cli_frames')} | {row.get('gui_audio') or 'none'} |"
            f" {row.get('cli_audio') or 'none'} | {'yes' if row.get('match') else '**no**'} |"
        )
    lines.append("")
    if failures:
        lines.append("Failures:")
        lines.extend(f"- {failure}" for failure in failures)
    else:
        lines.append(
            "The window's export runner and `subordinate-cli render` wrote the same"
            " number of frames and the same audio for every encoder above."
        )
    summary = "\n".join(lines) + "\n"
    (out / "summary.md").write_text(summary, encoding="utf-8")
    print(summary)
    for failure in failures:
        print(f"::error::GUI vs CLI: {failure}", flush=True)
    return 1 if failures and args.fail_on_error else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", default="subordinate-cli")
    parser.add_argument("--project", required=True, help="the project the editor has open")
    parser.add_argument("--sequence", default=None)
    parser.add_argument("--preset", default="youtube-1080p")
    parser.add_argument("--container", default="mp4")
    parser.add_argument("--encoders", nargs="+", default=["nvh264enc", "x264enc"])
    parser.add_argument("--frames", type=int, default=None)
    parser.add_argument("--out", required=True)
    parser.add_argument("--endpoint-dir", default="/tmp/subordinate-endpoint")
    parser.add_argument("--instance", default="default")
    parser.add_argument("--wait", type=int, default=120)
    parser.add_argument("--poll", type=int, default=1800)
    parser.add_argument("--gst-prefix", default="")
    parser.add_argument("--fail-on-error", action="store_true")
    return compare(parser.parse_args())


if __name__ == "__main__":
    sys.exit(main())
