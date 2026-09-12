"""What the two desktop flows share: staging, steps, and project vocabulary.

A flow is a list of named steps. Each one photographs the desktop when it
finishes, and a step that raises stops the run, photographs the failure,
prints a GitHub `::error::` annotation naming the step and its screenshot, and
writes `result.json` beside the pictures - which is how the workflow can say
which step failed and attach the picture of it (TASK-139 AC 2 and 5).
"""

from __future__ import annotations

import json
import os
import platform
import random
import shutil
import subprocess
import sys
import time
import traceback
import uuid
from dataclasses import dataclass, field
from pathlib import Path

from .session import DesktopError, Session

# --------------------------------------------------------------- the project

#: The colour tags every new project and media item carries (decision-3).
COLOR = {"space": "rec709", "transfer": "bt709", "primaries": "bt709"}


def uuid7() -> str:
    """A lowercase hyphenated UUIDv7, which is what every project id is.

    Minted here rather than asked for, because `media.import` and
    `timeline.add_clip` take the whole item: the command arrives with its own
    identity so that replaying it reproduces the project byte for byte
    (docs/schema/command-api.json).
    """
    milliseconds = int(time.time() * 1000)
    raw = bytearray(16)
    raw[0:6] = milliseconds.to_bytes(6, "big")
    raw[6:16] = random.randbytes(10)
    raw[6] = (raw[6] & 0x0F) | 0x70  # version 7
    raw[8] = (raw[8] & 0x3F) | 0x80  # variant
    return str(uuid.UUID(bytes=bytes(raw)))


def rational_time(value: int, rate: dict) -> dict:
    """An exact instant: a unit count at a rate, never a float."""
    return {"value": int(value), "rate": dict(rate)}


def media_item(name: str, relative_path: str) -> dict:
    """A `MediaItem` for a file already inside the project folder."""
    return {
        "id": uuid7(),
        "name": name,
        "path": relative_path,
        "proxy": "none",
        "offline": False,
        "color": dict(COLOR),
    }


def clip(name: str, media_id: str, start_units: int, duration_units: int, rate: dict) -> dict:
    """A `Clip` over `media_id`, with everything else at its default."""
    return {
        "id": uuid7(),
        "name": name,
        "media": media_id,
        "source_range": {
            "start": rational_time(start_units, rate),
            "duration": rational_time(duration_units, rate),
        },
        "opacity": 1000000,
        "transform": {
            "position": {"x": 0, "y": 0},
            "scale": {"x": 1000000, "y": 1000000},
            "rotation_degrees": 0,
        },
        "gain": 0,
        "fade_in": rational_time(0, rate),
        "fade_out": rational_time(0, rate),
        "markers": [],
    }


# ------------------------------------------------------------------- staging


@dataclass
class Stage:
    """The folder a flow works in, and what it found there."""

    directory: Path
    project: Path
    media: Path
    #: The media path as the project stores it: slash-separated and relative.
    media_relative: str
    sequence: str
    video_track: str
    audio_track: str
    frame_rate: dict


def stage(directory: Path | str, media: Path | str, cli: str) -> Stage:
    """Make a project folder holding a copy of the test clip and a new project.

    The media file is **copied** rather than linked: a media path is
    project-relative by model rule, and `MediaPath::relative_to` resolves
    symlinks, so a link to `/opt/subordinate/test-media` would be refused with
    `model.invalid_path`. The project file is made with `subordinate-cli new`,
    which starts it with one sequence and a video and an audio track - the same
    starter the editor's own File > New produces.
    """
    directory = Path(directory)
    if directory.exists():
        shutil.rmtree(directory, ignore_errors=True)
    directory.mkdir(parents=True, exist_ok=True)

    media = Path(media)
    if not media.exists():
        raise DesktopError(f"no test media at {media}")
    copy = directory / media.name
    shutil.copy2(media, copy)

    project = directory / "flow.sub"
    finished = subprocess.run(
        [cli, "new", str(project), "--name", "Desktop flow", "--force"],
        capture_output=True,
        text=True,
        check=False,
    )
    if finished.returncode != 0 or not project.exists():
        raise DesktopError(
            f"{cli} new failed ({finished.returncode}): "
            f"{finished.stderr.strip() or finished.stdout.strip()}"
        )

    document = json.loads(project.read_text(encoding="utf-8"))
    sequence = document["project"]["sequences"][0]
    video = next(
        track
        for track in sequence["tracks"]
        if _track_kind(track) == "video"
    )
    audio = next(
        (track for track in sequence["tracks"] if _track_kind(track) == "audio"),
        video,
    )
    return Stage(
        directory=directory,
        project=project,
        media=copy,
        media_relative=copy.name,
        sequence=sequence["id"],
        video_track=video["id"],
        audio_track=audio["id"],
        frame_rate=sequence["settings"]["frame_rate"],
    )


def _track_kind(track: dict) -> str:
    """A track's kind, however this schema version spells it."""
    kind = track.get("kind")
    if isinstance(kind, str):
        return kind.lower()
    if isinstance(kind, dict) and kind:
        return next(iter(kind)).lower()
    # Older files name the lane instead; V1 is video and A1 is audio.
    return "video" if str(track.get("name", "")).upper().startswith("V") else "audio"


def wait_for_command_api(session, log: Path | str, *, timeout: float = 120.0) -> str:
    """Wait until the editor has bound its endpoint, and say where.

    The window is on screen a second or two before the socket is: the editor
    binds off the UI thread and collects the outcome a frame or two later
    (crates/sub-ui/src/command_api.rs), so a bridge started at the moment the
    window appears is answered `command.not_running` and exits. The editor logs
    the outcome either way, which is the thing to wait for.
    """
    line = session.wait_for_log(
        log,
        r"the Command API is (listening on|not served)|another editor is already serving",
        timeout=timeout,
    )
    if "listening on" not in line:
        raise DesktopError(f"the editor is not serving the Command API: {line}")
    return line


# --------------------------------------------------------------------- steps


@dataclass
class Run:
    """One flow: its steps, its screenshots and its verdict."""

    name: str
    session: Session
    out: Path
    steps: list[dict] = field(default_factory=list)
    started: float = field(default_factory=time.monotonic)

    def step(self, label: str) -> "Step":
        return Step(self, label)

    def note(self, **facts) -> None:
        """Attach facts to the step that is running, or to the run."""
        if self.steps:
            self.steps[-1].setdefault("facts", {}).update(facts)

    def finish(self, error: BaseException | None = None) -> int:
        """Write result.json, summarise, and answer with the exit status."""
        failed = next((s for s in self.steps if s["state"] == "failed"), None)
        verdict = {
            "flow": self.name,
            "platform": platform.system().lower(),
            "seconds": round(time.monotonic() - self.started, 1),
            "state": "failed" if failed else "passed",
            "failed_step": failed["name"] if failed else None,
            "screenshot": failed.get("screenshot") if failed else None,
            "steps": self.steps,
        }
        if error is not None and failed is None:
            verdict["state"] = "failed"
            verdict["failed_step"] = "(outside any step)"
            verdict["error"] = str(error)
        (self.out / "result.json").write_text(json.dumps(verdict, indent=1))
        _summary(verdict, self.out)
        if verdict["state"] == "failed":
            name = verdict["failed_step"]
            shot = verdict.get("screenshot") or "(no screenshot)"
            print(
                f"::error title={self.name}: {name}::"
                f"step {name!r} failed; screenshot {shot}",
                flush=True,
            )
            return 1
        return 0


class Step:
    """One named step, photographed when it ends either way."""

    def __init__(self, run: Run, label: str) -> None:
        self.run = run
        self.label = label
        self.record: dict = {"name": label, "state": "running"}

    def __enter__(self) -> "Step":
        self.run.steps.append(self.record)
        self.started = time.monotonic()
        print(f"--- step: {self.label}", flush=True)
        return self

    def __exit__(self, kind, value, trace) -> bool:
        self.record["seconds"] = round(time.monotonic() - self.started, 1)
        self.record["state"] = "passed" if kind is None else "failed"
        if kind is not None:
            self.record["error"] = "".join(
                traceback.format_exception_only(kind, value)
            ).strip()
        try:
            shot = self.run.session.numbered_screenshot(self.label)
            self.record["screenshot"] = shot.name
        except Exception as error:  # noqa: BLE001 - a lost picture is not the failure
            self.record["screenshot_error"] = str(error)
        if kind is not None:
            print(f"    FAILED: {self.record['error']}", flush=True)
        else:
            print(f"    ok ({self.record['seconds']}s)", flush=True)
        return False

    def note(self, **facts) -> None:
        self.record.setdefault("facts", {}).update(facts)


def _summary(verdict: dict, out: Path) -> None:
    """Write the job summary table, when a job summary is what we are in."""
    lines = [
        f"### {verdict['flow']} on {verdict['platform']}: **{verdict['state']}**",
        "",
        "| step | state | seconds | screenshot |",
        "| --- | --- | --- | --- |",
    ]
    for step in verdict["steps"]:
        lines.append(
            f"| {step['name']} | {step['state']} | {step.get('seconds', '')} | "
            f"{step.get('screenshot', '')} |"
        )
    text = "\n".join(lines) + "\n"
    (out / "summary.md").write_text(text)
    target = os.environ.get("GITHUB_STEP_SUMMARY")
    if target:
        with open(target, "a", encoding="utf-8") as handle:
            handle.write(text)


def main(flow, name: str) -> int:
    """Run a flow function, writing its verdict however it ends."""
    from .session import open_session

    out = Path(os.environ.get("FLOW_OUT", "flow-out")).absolute()
    out.mkdir(parents=True, exist_ok=True)
    session = open_session(out / "shots")
    run = Run(name=name, session=session, out=out)
    error: BaseException | None = None
    try:
        flow(run)
    except BaseException as caught:  # noqa: BLE001 - every failure is reported
        error = caught
        traceback.print_exc()
    status = run.finish(error)
    print(json.dumps({"result": str(out / "result.json")}), flush=True)
    sys.stdout.flush()
    return status
