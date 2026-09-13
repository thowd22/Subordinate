#!/usr/bin/env python3
"""The MCP-only desktop flow (TASK-139 AC 3).

An agent-style script that acts **only** through `subordinate-mcp` while the
real editor window stands on the desktop: it opens a project, imports the
baked test clip, puts it on the timeline, splits it, renders it with the
vendor encoder and validates the file - photographing the desktop after every
step.

    FLOW_OUT=/tmp/flow python3 scripts/desktop/flow_mcp.py

Environment (all optional, and the workflow sets them):

| Variable | What it names |
| --- | --- |
| `FLOW_OUT` | where screenshots, logs and result.json go |
| `FLOW_WORK` | the project folder the flow builds |
| `FLOW_MEDIA` | the baked test clip |
| `FLOW_EDITOR`, `FLOW_CLI`, `FLOW_MCP` | the three binaries |
| `FLOW_PRESET` | the export preset (default `youtube-1080p`) |
| `FLOW_ENCODER` | the encoder element the export must use (default `nvh264enc`) |
| `FLOW_DISCOVERER` | the `gst-discoverer-1.0` that validates the output |

The editing session starts without a project file. Import uses an explicit
external reference, then the flow creates tracks, edits, saves and reopens.
The final export uses a separate headless bridge after closing the editor so
hardware decoder contention does not distort the export measurement.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parent))

from subdesktop import flow as flowlib  # noqa: E402
from subdesktop.mcp import Bridge, project_from_result as _project  # noqa: E402

WINDOW = "Subordinate"


def _env(name: str, default: str) -> str:
    return os.environ.get(name) or default


def run(run: flowlib.Run) -> None:
    out = run.out
    work = Path(_env("FLOW_WORK", str(out / "project")))
    media = Path(_env("FLOW_MEDIA", "/opt/subordinate/test-media/meld-4k60-excerpt-2min.mkv"))
    editor = _env("FLOW_EDITOR", "subordinate")
    cli = _env("FLOW_CLI", "subordinate-cli")
    mcp = _env("FLOW_MCP", "subordinate-mcp")
    preset = _env("FLOW_PRESET", "youtube-1080p")
    encoder = _env("FLOW_ENCODER", "nvh264enc")
    discoverer = _env("FLOW_DISCOVERER", "subordinate-gst-discoverer")

    with run.step("prepare a fresh unsaved project") as step:
        work = work.resolve()
        work.mkdir(parents=True, exist_ok=True)
        project = work / ("flow-" + flowlib.uuid7() + ".sub")
        media = media.resolve(strict=True)
        staged = SimpleNamespace(directory=work, project=project, media=media,
                                 sequence=None, video_track=None,
                                 frame_rate={"numerator": 24, "denominator": 1})
        if project.exists():
            raise AssertionError("the flow must start before the first save")
        step.note(project=str(project), external_media=str(media),
                  editor=editor, cli=cli, mcp=mcp,
                  requested_ref=os.environ.get("FLOW_REQUESTED_REF", "local"),
                  installed_release=os.environ.get("FLOW_INSTALLED_RELEASE", "unknown"),
                  tested_ref=os.environ.get("FLOW_TESTED_REF", "unknown"),
                  provenance="current artifact" if os.environ.get("FLOW_TESTED_REF") else "unverified local/baked binaries")

    app_log = out / "editor.log"
    with run.step("launch the editor on the desktop") as step:
        started = run.session.launch(
            [editor], log=app_log, cwd=str(staged.directory)
        )
        window = run.session.wait_for_title(WINDOW, timeout=180)
        window = run.session.maximize(window)
        endpoint = flowlib.wait_for_command_api(run.session, app_log)
        step.note(pid=started.pid, window=window.as_dict(), endpoint=endpoint)
        # Every screenshot from here on wakes the window first: the edits below
        # arrive over a socket, and an idle egui window paints nothing, so the
        # picture would show the project as it was rather than as the agent
        # has just made it (run 34672010010).
        run.before_shot = lambda: run.session.nudge(window)

    rate = staged.frame_rate
    fps = rate["numerator"] / rate["denominator"]
    # Four seconds of the two-minute clip, and a cut two seconds in: enough to
    # be a real 4K60 decode and short enough that the render is seconds.
    length = int(round(fps * 4))
    cut = int(round(fps * 2))
    item = flowlib.media_item(staged.media.name, "unused")
    item["path"] = {"external": str(staged.media)}
    piece = flowlib.clip("Meld", item["id"], 0, length, rate)

    with run.step("connect the bridge to the editor's own endpoint") as step:
        bridge = Bridge(
            mcp,
            cwd=staged.directory,
            env={"SUBORDINATE_MCP_NO_LAUNCH": "1"},
            require_editor=True,
            log=out / "mcp-editor.log",
        )
        step.note(connection=bridge.connection)
    try:
        with run.step("project.new without opening or saving a file") as step:
            bridge.call("project.new", {"name": "Desktop MCP flow"})
            if _project(bridge.call("project.get", {})).get("sequences"):
                raise AssertionError("project.new must start with no sequences")
            state = bridge.call("project.get", {})
            step.note(connection=bridge.connection, project=_name_of(state))

        with run.step("media.import of the baked test clip") as step:
            bridge.call("media.import", {"item": item})
            listed = bridge.call("media.list", {})
            names = json.dumps(listed)
            if item["id"] not in names:
                raise AssertionError(f"the imported item is not in media.list: {names[:400]}")
            step.note(media=item["id"], path=item["path"])

        with run.step("create the first sequence and tracks through MCP") as step:
            bridge.call("sequence.create", {"name": "Main", "settings": {
                "resolution": {"width": 1920, "height": 1080},
                "frame_rate": rate, "sample_rate": 48000, "color": flowlib.COLOR}})
            sequence = _project(bridge.call("project.get", {}))["sequences"][0]
            staged.sequence = sequence["id"]
            for name, kind in (("V1", "video"), ("A1", "audio")):
                bridge.call("track.add", {"sequence": staged.sequence, "name": name, "kind": kind})
            tracks = _find_tracks(bridge.call("timeline.get_state", {"sequence": staged.sequence}))
            staged.video_track = next(t["id"] for t in tracks if t["kind"] == "video")
            step.note(sequence=staged.sequence, tracks=len(tracks))

        with run.step("timeline.add_clip") as step:
            bridge.call(
                "timeline.add_clip",
                {
                    "sequence": staged.sequence,
                    "track": staged.video_track,
                    "start": flowlib.rational_time(0, rate),
                    "clip": piece,
                },
            )
            clips = _clips(bridge, staged.sequence)
            if len(clips) != 1:
                raise AssertionError(f"expected one clip on the track, found {len(clips)}")
            step.note(clip=piece["id"], clips=len(clips))

        with run.step("timeline.split_clip") as step:
            bridge.call(
                "timeline.split_clip",
                {
                    "sequence": staged.sequence,
                    "track": staged.video_track,
                    "clip": piece["id"],
                    "at": flowlib.rational_time(cut, rate),
                },
            )
            clips = _clips(bridge, staged.sequence)
            if len(clips) != 2:
                raise AssertionError(f"the split left {len(clips)} clips, not 2")
            step.note(clips=len(clips), at_frame=cut)

        with run.step("project.save, so the render sees the edit") as step:
            bridge.call("project.save", {"path": str(staged.project)})
            bridge.call("project.new", {"name": "Reopen check"})
            bridge.call("project.open", {"path": str(staged.project)})
            listed = bridge.call("media.list", {})
            if str(staged.media) not in _external_paths(listed):
                raise AssertionError("save/reopen lost the external media reference")
            if len(_clips(bridge, staged.sequence)) != 2:
                raise AssertionError("save/reopen lost the timeline edit")
            step.note(bytes=staged.project.stat().st_size, media=listed)

        with run.step("the window still holds what the agent edited") as step:
            window = run.session.wait_for_title(WINDOW, timeout=30)
            clips = _clips(bridge, staged.sequence)
            step.note(window=window.as_dict(), clips=len(clips))
    finally:
        bridge.close()

    # Close the editor before the headless export because on
    # Windows the two compete for the GPU's decoder: with the window still open
    # on the same 4K60 clip, the export managed one frame in fifteen minutes
    # (run 34672165182), and finished in seconds once it had the machine to
    # itself. Every edit above was made, asserted and photographed while the
    # window was up; this is the encode.
    with run.step("close the editor before rendering") as step:
        run.session.stop(started)
        run.before_shot = None
        step.note(closed=started.pid)

    output = staged.directory / "flow-export.mp4"
    render_log = out / "mcp-export.log"
    with run.step("export.render with the vendor encoder") as step:
        # A bridge of its own: a scratch instance so it cannot collide with the
        # editor's endpoint, and no NO_LAUNCH, so it starts the
        # `subordinate-cli serve` that serves the export family. Its working
        # directory is the project folder, which is what the serving process
        # resolves relative media paths against.
        export = Bridge(
            mcp,
            cwd=staged.directory,
            env={
                "SUBORDINATE_INSTANCE": "desktop-flow-export",
                "SUBORDINATE_ENDPOINT_DIR": str(staged.directory / "endpoint"),
                "SUBORDINATE_CLI": cli,
                # The encoder that ran is a debug line of the export pipeline's
                # own, and it is the only place the element name appears.
                "SUBORDINATE_LOG": "info,sub_export=debug",
            },
            require_editor=False,
            log=render_log,
        )
        try:
            export.call("project.open", {"path": str(staged.project)})
            presets = export.call("export.list_presets", {})
            ids = _preset_ids(presets)
            if preset not in ids:
                raise AssertionError(f"{preset} is not an export preset; this build has {ids}")
            status = export.call(
                "export.render",
                {"preset": preset, "sequence": staged.sequence, "output": str(output)},
            )
            job = status["job"]
            status = _await_export(export, job)
            step.note(job=job, state=status["state"], frames=status.get("frames_total"))
            if status["state"] != "completed":
                raise AssertionError(f"the export ended {status['state']}: {json.dumps(status)[:500]}")
        finally:
            export.close()

    with run.step("validate the exported file") as step:
        if not output.exists() or output.stat().st_size == 0:
            raise AssertionError(f"no file at {output}")
        probe = subprocess.run(
            [discoverer, str(output)], capture_output=True, text=True, check=False
        )
        text = probe.stdout + probe.stderr
        (out / "export-probe.txt").write_text(text)
        if "H.264" not in text and "h264" not in text.lower():
            raise AssertionError(f"the export is not H.264:\n{text[:800]}")
        used = _encoder_used(render_log)
        step.note(bytes=output.stat().st_size, encoder=used or "(not logged)")
        if used != encoder:
            raise AssertionError(
                f"the export ran on {used!r}, not the vendor encoder {encoder!r}"
            )

    run.session.stop(started)


def _external_paths(value):
    if isinstance(value, dict):
        result = [value["external"]] if isinstance(value.get("external"), str) else []
        return result + [path for child in value.values() for path in _external_paths(child)]
    if isinstance(value, list):
        return [path for child in value for path in _external_paths(child)]
    return []


def _name_of(state) -> str:
    return str(_project(state).get("name", ""))


def _clips(bridge: Bridge, sequence: str) -> list:
    """Every clip on the first track of `sequence`, as the timeline reports it."""
    state = bridge.call("timeline.get_state", {"sequence": sequence})
    text = json.dumps(state)
    tracks = _find_tracks(state)
    for track in tracks:
        items = track.get("items") or track.get("children") or []
        clips = [item for item in items if isinstance(item, dict) and "clip" in item]
        if clips:
            return clips
    # A timeline with no clip at all is a real answer, not a parse failure.
    if '"clip"' not in text:
        return []
    raise AssertionError(f"could not read the clips out of timeline.get_state: {text[:600]}")


def _find_tracks(state) -> list:
    if isinstance(state, dict):
        if isinstance(state.get("tracks"), list):
            return state["tracks"]
        for value in state.values():
            found = _find_tracks(value)
            if found:
                return found
    return []


def _preset_ids(presets) -> list[str]:
    if isinstance(presets, dict):
        presets = presets.get("presets", presets)
    if isinstance(presets, list):
        return [p.get("id") or p.get("name") for p in presets if isinstance(p, dict)]
    return []


def _await_export(bridge: Bridge, job: str, *, timeout: float = 420.0) -> dict:
    import time

    deadline = time.monotonic() + timeout
    status = {"state": "running"}
    while time.monotonic() < deadline:
        status = bridge.call("export.progress", {"job": job})
        if status.get("state") != "running":
            return status
        time.sleep(2)
    return status


def _encoder_used(log: Path) -> str | None:
    """The element the export pipeline started with, from the bridge's log."""
    if not log.exists():
        return None
    for line in log.read_text(encoding="utf-8", errors="replace").splitlines():
        if "export pipeline started" in line or "video_encoder" in line:
            for part in line.replace('"', " ").replace("=", " ").split():
                if part.endswith("enc") and part not in {"video_encoder", "audio_encoder"}:
                    return part
    return None


if __name__ == "__main__":
    sys.exit(flowlib.main(run, "MCP flow"))
