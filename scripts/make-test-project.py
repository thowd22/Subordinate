#!/usr/bin/env python3
"""Write a project file over one media file, for a headless render on a runner.

The desktop flows build their project through the Command API; a GPU job that
only wants to *render* something has no window and no bridge, so this makes the
same document directly: `subordinate-cli new` writes the starter project, and
this fills in one media item, one video clip and as many audio clips as asked
for, then stamps the sequence's canvas and timebase.

The media path a project stores is relative to the project file, so the
project is written **next to the media** rather than the media copied.

    python3 scripts/make-test-project.py \
        --cli subordinate-cli --media C:/SubordinateTest/meld.mkv \
        --out C:/SubordinateTest/render.sub \
        --resolution 3840x2160 --frame-rate 60 --audio-tracks 3 --frames 48

It prints the project path, the sequence name and the track count as JSON.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import subprocess
import sys
import time
import uuid
from pathlib import Path

#: The colour tags every new project and media item carries (decision-3).
COLOR = {"space": "rec709", "transfer": "bt709", "primaries": "bt709"}


def uuid7() -> str:
    """A lowercase hyphenated UUIDv7, which is what every project id is."""
    milliseconds = int(time.time() * 1000)
    raw = bytearray(16)
    raw[0:6] = milliseconds.to_bytes(6, "big")
    raw[6:16] = random.randbytes(10)
    raw[6] = (raw[6] & 0x0F) | 0x70  # version 7
    raw[8] = (raw[8] & 0x3F) | 0x80  # variant
    return str(uuid.UUID(bytes=bytes(raw)))


def rational(value: int, rate: dict) -> dict:
    """An exact instant: a unit count at a rate, never a float."""
    return {"value": int(value), "rate": dict(rate)}


def clip(name: str, media_id: str, frames: int, rate: dict) -> dict:
    """A clip over the whole of `media_id`, with everything else at default."""
    return {
        "id": uuid7(),
        "name": name,
        "media": media_id,
        "source_range": {
            "start": rational(0, rate),
            "duration": rational(frames, rate),
        },
        "opacity": 1_000_000,
        "transform": {
            "position": {"x": 0, "y": 0},
            "scale": {"x": 1_000_000, "y": 1_000_000},
            "rotation_degrees": 0,
        },
        "gain": 0,
        "fade_in": rational(0, rate),
        "fade_out": rational(0, rate),
        "markers": [],
    }


def track(kind: str, name: str, items: list) -> dict:
    return {
        "id": uuid7(),
        "name": name,
        "kind": kind,
        "items": items,
        "gain": 0,
        "muted": False,
        "solo": False,
        "locked": False,
    }


def kind_of(entry: dict) -> str:
    """A track's kind, however this schema version spells it."""
    kind = entry.get("kind")
    if isinstance(kind, str):
        return kind.lower()
    if isinstance(kind, dict) and kind:
        return next(iter(kind)).lower()
    return "video" if str(entry.get("name", "")).upper().startswith("V") else "audio"


def parse_resolution(text: str) -> tuple[int, int]:
    width, _, height = text.lower().partition("x")
    return int(width), int(height)


def parse_rate(text: str) -> dict:
    """`60`, `60000/1001` or `30/1` as the exact rational the model stores."""
    numerator, _, denominator = text.partition("/")
    return {
        "numerator": int(numerator),
        "denominator": int(denominator) if denominator else 1,
    }


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", default=os.environ.get("FLOW_CLI", "subordinate-cli"))
    parser.add_argument("--media", required=True, help="the media file to lay down")
    parser.add_argument("--out", required=True, help="where the project is written")
    parser.add_argument("--name", default="Render test")
    parser.add_argument("--resolution", default=None, help="WIDTHxHEIGHT canvas")
    parser.add_argument("--frame-rate", default=None, help="e.g. 60 or 60000/1001")
    parser.add_argument(
        "--audio-tracks",
        type=int,
        default=0,
        help="how many audio tracks carry a clip of the same media",
    )
    parser.add_argument(
        "--frames",
        type=int,
        default=600,
        help="clip length in frames of the sequence timebase",
    )
    args = parser.parse_args(argv)

    media = Path(args.media).resolve()
    if not media.exists():
        raise SystemExit(f"no media at {media}")
    project = Path(args.out).resolve()
    if project.parent != media.parent:
        raise SystemExit(
            f"the project must sit beside its media: {project.parent} != {media.parent}"
        )

    finished = subprocess.run(
        [args.cli, "new", str(project), "--name", args.name, "--force"],
        capture_output=True,
        text=True,
        check=False,
    )
    if finished.returncode != 0 or not project.exists():
        raise SystemExit(
            f"{args.cli} new failed ({finished.returncode}): "
            f"{finished.stderr.strip() or finished.stdout.strip()}"
        )

    document = json.loads(project.read_text(encoding="utf-8"))
    body = document["project"]
    sequence = body["sequences"][0]
    settings = sequence["settings"]
    if args.resolution:
        width, height = parse_resolution(args.resolution)
        settings["resolution"] = {"width": width, "height": height}
    if args.frame_rate:
        settings["frame_rate"] = parse_rate(args.frame_rate)
    rate = settings["frame_rate"]

    item = {
        "id": uuid7(),
        "name": media.stem,
        "path": media.name,
        "proxy": "none",
        "offline": False,
        "color": dict(COLOR),
    }
    body.setdefault("media", []).append(item)

    video = next(t for t in sequence["tracks"] if kind_of(t) == "video")
    video["items"] = [{"clip": clip(media.stem, item["id"], args.frames, rate)}]

    audio = [t for t in sequence["tracks"] if kind_of(t) == "audio"]
    for index in range(args.audio_tracks):
        name = f"A{index + 1}"
        piece = {"clip": clip(f"{media.stem} {name}", item["id"], args.frames, rate)}
        if index < len(audio):
            audio[index]["items"] = [piece]
        else:
            sequence["tracks"].append(track("audio", name, [piece]))
    for spare in audio[args.audio_tracks :]:
        spare["items"] = []

    project.write_text(json.dumps(document, indent=2, sort_keys=True), encoding="utf-8")
    print(
        json.dumps(
            {
                "project": str(project),
                "media": media.name,
                "sequence": sequence.get("name"),
                "resolution": settings["resolution"],
                "frame_rate": settings["frame_rate"],
                "tracks": len(sequence["tracks"]),
                "audio_tracks": args.audio_tracks,
                "frames": args.frames,
            }
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
