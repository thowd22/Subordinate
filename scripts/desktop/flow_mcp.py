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

**Why the render is a second bridge session.** Every edit below lands in the
window on screen: the editor binds the Command API at startup and the bridge
connects to it, with `SUBORDINATE_MCP_NO_LAUNCH=1` so a session that cannot
reach the window fails instead of quietly editing a project nobody can see.
The `export.*` family, though, is served by the *process* that owns the
decoders and the encoder (docs/schema/host-api.json), and only
`subordinate-cli serve` installs it - the editor's endpoint serves the engine's
own methods and the plugin methods, and answers `core.method_not_found` for
`export.render`. So the flow saves the project the window is holding and makes
its last two calls through a second bridge, on a scratch instance of its own,
against the file it just wrote. It is still MCP and nothing else; it is simply
the only process on the machine that serves an export.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from subdesktop import flow as flowlib  # noqa: E402
from subdesktop.mcp import Bridge  # noqa: E402

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

    with run.step("stage the project folder") as step:
        staged = flowlib.stage(work, media, cli)
        step.note(
            project=str(staged.project),
            media=staged.media_relative,
            sequence=staged.sequence,
            track=staged.video_track,
            frame_rate=staged.frame_rate,
        )

    app_log = out / "editor.log"
    with run.step("launch the editor on the desktop") as step:
        started = run.session.launch(
            [editor, str(staged.project)], log=app_log, cwd=str(staged.directory)
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
    item = flowlib.media_item(staged.media.name, staged.media_relative)
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
        with run.step("project.new and project.open through the bridge") as step:
            bridge.call("project.new", {"name": "Desktop MCP flow"})
            bridge.call("project.open", {"path": str(staged.project)})
            state = bridge.call("project.get", {})
            step.note(connection=bridge.connection, project=_name_of(state))

        with run.step("media.import of the baked test clip") as step:
            bridge.call("media.import", {"item": item})
            listed = bridge.call("media.list", {})
            names = json.dumps(listed)
            if item["id"] not in names:
                raise AssertionError(f"the imported item is not in media.list: {names[:400]}")
            step.note(media=item["id"], path=item["path"])

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
            step.note(bytes=staged.project.stat().st_size)

        with run.step("the window still holds what the agent edited") as step:
            window = run.session.wait_for_title(WINDOW, timeout=30)
            clips = _clips(bridge, staged.sequence)
            step.note(window=window.as_dict(), clips=len(clips))
    finally:
        bridge.close()

    # The render is a process of its own either way - the editor does not serve
    # the export family - and the editor is closed before it starts because on
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


def _name_of(state) -> str:
    if isinstance(state, dict):
        project = state.get("project", state)
        if isinstance(project, dict):
            return str(project.get("name", ""))
    return ""


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
