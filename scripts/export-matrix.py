#!/usr/bin/env python3
"""The export matrix: every encoder a machine carries, every preset, every source.

TASK-143. Export had been verified one encoder at a time - `nvh264enc` on the
T4, `vah264enc` on box, an AMF smoke on the user's desktop, `x264enc` in CI -
and never as a matrix, never for HEVC or AV1, and never with the outputs
checked beyond "the file has an H.264 stream in it". This script is the matrix,
and it is one script rather than six copies of a workflow step because every
machine has to be measured the same way for the table to mean anything.

What it does, in order:

1. **Discovery.** `subordinate-cli diag` carries the exporter's own encoder
   probe: for every catalogued element, whether the factory is registered,
   whether an instance reaches READY, and whether this machine ranks it NONE.
   That is the authority on "present", because it is what the exporter itself
   will ask. `gst-inspect-1.0 --exists` is recorded next to it as the raw
   registry answer, so a disagreement between the two is visible rather than
   silent.
2. **The case list.** Every encoder that is *pinnable* here (present and
   READY - a NONE rank is not a reason to skip, it is exactly what `--encoder`
   overrides) crossed with every preset whose video codec it produces, crossed
   with every source project.
3. **The render.** `subordinate-cli render --encoder E --preset P --verify`,
   with the report kept whole.
4. **The validation.** Not the CLI's own word: the file is read back with
   `gst-discoverer-1.0` for its stream layout and duration, and *decoded* to
   count its frames, which is the only check that catches an encoder that
   writes a valid header and drops half the picture.
5. **The table.** One row per machine x source x preset x encoder with
   pass/fail, size, frames and the failure's own words, as Markdown for the job
   summary and as JSON for anything that wants to read it back.

Failing outputs are kept (with their discoverer reports) and passing ones are
deleted: a 4K60 matrix writes more than an artifact should carry, and a file
that passed has nothing left to say.

Subcommands:

    export-matrix.py run        the matrix
    export-matrix.py project    build a .sub around one media file
    export-matrix.py discover   the encoder discovery alone, as JSON

Every subcommand prints JSON or Markdown and writes nothing outside `--out`.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass, field, asdict
from pathlib import Path
from typing import Any

# --------------------------------------------------------------------------
# The encoder catalogue, mirroring sub_export::encoder::CATALOGUE.
#
# It is duplicated here rather than read out of the Rust source because this
# script also runs against a *released* binary on the desktop image, whose
# catalogue is whatever that release shipped. `diag` tells us what that build
# really knows; this list is only the order and the codec mapping used when a
# machine's `diag` is too old to carry the probe at all.
# --------------------------------------------------------------------------
CATALOGUE: list[tuple[str, str]] = [
    ("nvh264enc", "h264"),
    ("nvh265enc", "h265"),
    ("nvav1enc", "av1"),
    ("vah264enc", "h264"),
    ("vah265enc", "h265"),
    ("vaav1enc", "av1"),
    ("amfh264enc", "h264"),
    ("amfh265enc", "h265"),
    ("amfav1enc", "av1"),
    ("vtenc_h264", "h264"),
    ("vtenc_h265", "h265"),
    ("mfh264enc", "h264"),
    ("mfh265enc", "h265"),
    ("x264enc", "h264"),
    ("x265enc", "h265"),
    ("svtav1enc", "av1"),
    ("av1enc", "av1"),
    ("rav1enc", "av1"),
]

# The presets this script renders with when `--presets` names none, and the
# codec each one asks for. `audio-only` is deliberately absent: it carries no
# video stream, so `render` refuses it with `export.unsupported_combination`
# before any encoder is chosen (see the note the summary prints).
DEFAULT_PRESETS: dict[str, tuple[str, str]] = {
    "youtube-1080p": ("h264", "mp4"),
    "youtube-4k": ("h264", "mp4"),
    "mezzanine": ("h264", "mkv"),
    "h265-archive": ("h265", "mkv"),
    "av1-archive": ("av1", "mkv"),
}

# Encoders slow enough that a cell of the normal length would outlast the job.
# libaom's `av1enc` is seconds to minutes a frame at its default `cpu-used`,
# and rav1e and SVT-AV1 are not much better on a hosted runner, so their cells
# render `--slow-frames` frames instead. The count used is in the table.
SLOW_ENCODERS: tuple[str, ...] = ("av1enc", "rav1enc", "svtav1enc")

# How the codec a preset asks for shows up in a discoverer report.
CODEC_PATTERNS: dict[str, re.Pattern[str]] = {
    "h264": re.compile(r"h\.?264|avc", re.I),
    "h265": re.compile(r"h\.?265|hevc", re.I),
    "av1": re.compile(r"\bav1\b", re.I),
}


def vendor_of(element: str) -> str:
    """The backend an element belongs to, from its name.

    `diag` reports the vendor itself; this is the fallback for a build whose
    catalogue predates the element, and it only has to be right about which
    encoders are software, because that is the one thing the case list filters
    on.
    """
    for prefix, vendor in (
        ("nv", "nvenc"),
        ("va", "va"),
        ("amf", "amf"),
        ("vtenc", "videotoolbox"),
        ("mf", "mediafoundation"),
    ):
        if element.startswith(prefix):
            return vendor
    return "software"


def gst_tool(name: str, prefix: str | None) -> str:
    """The GStreamer tool to run, honouring a wrapper prefix.

    The Linux desktop image has no system GStreamer at all: its tools are
    `subordinate-gst-inspect-1.0` and friends, wrappers round the AppImage's
    own bundled 1.28. `--gst-prefix subordinate-` makes every call in this
    script go through them, so the runtime that is measured is the runtime the
    editor actually exports with.
    """
    tool = f"{prefix}{name}" if prefix else name
    # Windows needs the extension spelled out. CreateProcess appends `.exe`
    # only when the name has no extension at all, and every GStreamer tool is
    # called `...-1.0` - Windows reads `.0` as the extension, searches for a
    # file by that exact name and reports "the system cannot find the file
    # specified" from a directory that is on PATH and does hold the binary
    # (TASK-143, yodaddy, run 34682535386).
    if os.name == "nt":
        tool += ".exe"
    return tool


@dataclass
class Run:
    """One finished subprocess."""

    code: int
    out: str
    err: str
    seconds: float

    @property
    def ok(self) -> bool:
        return self.code == 0


def run(argv: list[str], timeout: int = 1800, env: dict[str, str] | None = None) -> Run:
    """Runs `argv`, capturing both streams and never raising on a failure.

    A non-zero exit is a result here, not an exception: the whole point of the
    matrix is to record what failed and carry on to the next case.
    """
    started = time.monotonic()
    merged = dict(os.environ)
    if env:
        merged.update(env)
    try:
        done = subprocess.run(
            argv,
            capture_output=True,
            text=True,
            errors="replace",
            timeout=timeout,
            env=merged,
        )
        return Run(done.returncode, done.stdout, done.stderr, time.monotonic() - started)
    except subprocess.TimeoutExpired as expired:
        # Whatever it managed to say before it stopped saying anything is the
        # most useful thing about a timeout: `render` prints a progress line
        # per frame on stderr, so the tail of it is the frame the encoder
        # stalled on (box's vah264enc at 4K, run 34674014655).
        def text(stream: object) -> str:
            if isinstance(stream, str):
                return stream
            if isinstance(stream, (bytes, bytearray)):
                return stream.decode("utf-8", "replace")
            return ""

        return Run(
            124,
            text(expired.stdout),
            f"timed out after {timeout}s; last output: {text(expired.stderr)[-1500:]}",
            time.monotonic() - started,
        )
    except OSError as error:
        return Run(127, "", f"{argv[0]}: {error}", time.monotonic() - started)


def uri_of(path: Path) -> str:
    """A `file://` URI GStreamer takes on both platforms."""
    return path.resolve().as_uri()


# --------------------------------------------------------------------------
# Discovery
# --------------------------------------------------------------------------


def discover(cli: str, gst_prefix: str | None) -> dict[str, Any]:
    """What this machine can encode with, from the exporter and the registry."""
    inspect = gst_tool("gst-inspect-1.0", gst_prefix)
    registry: dict[str, bool] = {}
    for element, _codec in CATALOGUE:
        registry[element] = run([inspect, "--exists", element], timeout=120).ok

    diag = run([cli, "diag", "--compact"], timeout=600)
    probe: list[dict[str, Any]] = []
    diag_error: str | None = None
    if diag.ok:
        try:
            document = json.loads(diag.out)
            probe = document.get("encoder_probe", {}).get("encoders", []) or []
            if not probe and isinstance(document.get("encoder_probe"), list):
                probe = document["encoder_probe"]
        except json.JSONDecodeError as error:
            diag_error = f"diag did not print JSON: {error}"
    else:
        diag_error = (diag.err or diag.out).strip()[-2000:]

    by_element = {entry.get("element"): entry for entry in probe if isinstance(entry, dict)}
    encoders = []
    for element, codec in CATALOGUE:
        entry = by_element.get(element, {})
        encoders.append(
            {
                "element": element,
                "codec": entry.get("codec", codec),
                "vendor": entry.get("vendor") or vendor_of(element),
                "in_registry": registry[element],
                # `present` and `ready` come from the exporter's own probe when
                # it has one; a build whose catalogue predates this element
                # reports nothing for it, and then the registry is all we have.
                "present": bool(entry.get("present", registry[element])),
                "ready": bool(entry.get("ready", registry[element])),
                "deranked": bool(entry.get("deranked", False)),
                "detail": entry.get("detail"),
                "catalogued": element in by_element,
            }
        )
    version = run([inspect, "--version"], timeout=120)
    return {
        "platform": platform.platform(),
        "python": sys.version.split()[0],
        "gstreamer": version.out.strip().splitlines()[0] if version.ok and version.out else None,
        "diag_error": diag_error,
        "encoders": encoders,
    }


def pinnable(discovery: dict[str, Any]) -> list[dict[str, Any]]:
    """The encoders an export may be pinned to here.

    A `NONE` rank does not disqualify one: every VA-API encoder ships ranked
    `NONE` by design, and naming an element is exactly the decision that rank
    exists to withhold (`EncoderStatus::is_pinnable`).
    """
    return [e for e in discovery["encoders"] if e["present"] and e["ready"]]


# --------------------------------------------------------------------------
# Reading a media file
# --------------------------------------------------------------------------

_DURATION = re.compile(r"^\s*Duration:\s*(\d+):(\d\d):(\d\d)\.(\d+)")
_VIDEO = re.compile(r"^\s*video\s*#\d+:\s*(.+?)\s*$")
_AUDIO = re.compile(r"^\s*audio\s*#\d+:\s*(.+?)\s*$")
_WIDTH = re.compile(r"^\s*Width:\s*(\d+)")
_HEIGHT = re.compile(r"^\s*Height:\s*(\d+)")
_RATE = re.compile(r"^\s*Frame rate:\s*(\d+)/(\d+)")
_CHANNELS = re.compile(r"^\s*Channels:\s*(\d+)")
_SAMPLE = re.compile(r"^\s*Sample rate:\s*(\d+)")


@dataclass
class Stream:
    """One stream of a probed file."""

    kind: str
    codec: str
    width: int | None = None
    height: int | None = None
    rate: tuple[int, int] | None = None
    channels: int | None = None
    sample_rate: int | None = None


@dataclass
class Probe:
    """What the discoverer said about a file."""

    path: str
    seconds: float | None
    streams: list[Stream] = field(default_factory=list)
    text: str = ""
    ok: bool = True
    error: str | None = None

    @property
    def video(self) -> list[Stream]:
        return [s for s in self.streams if s.kind == "video"]

    @property
    def audio(self) -> list[Stream]:
        return [s for s in self.streams if s.kind == "audio"]


def probe(path: Path, gst_prefix: str | None, timeout: int = 600) -> Probe:
    """Reads `path` with `gst-discoverer-1.0` and parses its report.

    The text is kept whole: it is the artifact a failure is diagnosed from, and
    the parse below only lifts out what the matrix asserts on.
    """
    tool = gst_tool("gst-discoverer-1.0", gst_prefix)
    done = run([tool, uri_of(path)], timeout=timeout)
    result = Probe(path=str(path), seconds=None, text=done.out + done.err)
    if not done.ok:
        result.ok = False
        result.error = (done.err or done.out).strip()[-2000:]
        return result
    current: Stream | None = None
    for line in result.text.splitlines():
        match = _DURATION.match(line)
        if match and result.seconds is None:
            hours, minutes, seconds, fraction = match.groups()
            result.seconds = (
                int(hours) * 3600
                + int(minutes) * 60
                + int(seconds)
                + float("0." + fraction)
            )
            continue
        match = _VIDEO.match(line)
        if match:
            current = Stream("video", match.group(1))
            result.streams.append(current)
            continue
        match = _AUDIO.match(line)
        if match:
            current = Stream("audio", match.group(1))
            result.streams.append(current)
            continue
        if current is None:
            continue
        for pattern, apply in (
            (_WIDTH, lambda m: setattr(current, "width", int(m.group(1)))),
            (_HEIGHT, lambda m: setattr(current, "height", int(m.group(1)))),
            (_RATE, lambda m: setattr(current, "rate", (int(m.group(1)), int(m.group(2))))),
            (_CHANNELS, lambda m: setattr(current, "channels", int(m.group(1)))),
            (_SAMPLE, lambda m: setattr(current, "sample_rate", int(m.group(1)))),
        ):
            match = pattern.match(line)
            if match:
                apply(match)
                break
    return result


_CHAIN = re.compile(r"chain\s+\*+")


# Which demuxer opens which container. The counter reads the *muxed* video
# track rather than decoding it, so nothing but the demuxer has to be installed.
DEMUXERS: dict[str, str] = {
    ".mp4": "qtdemux",
    ".mov": "qtdemux",
    ".m4v": "qtdemux",
    ".mkv": "matroskademux",
    ".webm": "matroskademux",
}


def count_frames(path: Path, gst_prefix: str | None, timeout: int = 600) -> tuple[int | None, str]:
    """Counts the video frames in `path` and says how it counted them.

    This is the check the CLI's own `--verify` cannot make: the discoverer
    reads headers, and an encoder that writes a plausible header and then drops
    frames passes every header check there is. `identity silent=false` prints
    one `chain` line per buffer and `gst-launch -v` lets those lines out, so
    counting them counts frames.

    The first attempt reads the muxed video track straight off the demuxer:
    one buffer per coded frame, and nothing but the demuxer needs to be
    installed. That matters more than it sounds - the hosted runner has no AAC
    *decoder*, so a full `decodebin` of an MP4 this matrix wrote failed to
    preroll on the audio track and counted nothing at all (run 34672568992).
    The audio pad is deliberately left unlinked: a demuxer's flow combiner only
    errors when *every* pad is unlinked, so the video branch carries the
    pipeline on its own.

    Decoding is the fallback, for a container with no entry in `DEMUXERS`.
    Returns `(None, reason)` when neither could run, so an unknown count reads
    as unknown rather than as zero.
    """
    launch = gst_tool("gst-launch-1.0", gst_prefix)
    # Forward slashes: gst-launch's own parser treats a backslash as an escape,
    # so a Windows path in `location=` arrives mangled and the pipeline fails
    # to build (TASK-143, yodaddy, run 34683736400). GStreamer takes either
    # separator on Windows.
    location = str(path.resolve()).replace("\\", "/")
    attempts: list[tuple[str, list[str]]] = []
    demuxer = DEMUXERS.get(path.suffix.lower())
    if demuxer:
        attempts.append(
            (
                f"{demuxer} video track",
                [
                    launch,
                    "-v",
                    "filesrc",
                    f"location={location}",
                    "!",
                    demuxer,
                    "name=d",
                    "d.video_0",
                    "!",
                    "identity",
                    "silent=false",
                    "!",
                    "fakesink",
                    "sync=false",
                ],
            )
        )
    attempts.append(
        (
            "decoded frames",
            [
                launch,
                "-v",
                "uridecodebin",
                f"uri={uri_of(path)}",
                "caps=video/x-raw",
                "expose-all-streams=false",
                "!",
                "identity",
                "silent=false",
                "!",
                "fakesink",
                "sync=false",
            ],
        )
    )
    last = ""
    for how, argv in attempts:
        done = run(argv, timeout=timeout)
        count = len(_CHAIN.findall(done.out))
        if done.ok and count:
            return count, how
        last = (done.err or done.out).strip()[-300:]
    return None, cell(f"the frame counter could not read the file: {last}")


# --------------------------------------------------------------------------
# Building a project around one media file
# --------------------------------------------------------------------------


def time_value(value: int, rate: tuple[int, int]) -> dict[str, Any]:
    """A `RationalTime` as the project file writes it."""
    return {"rate": {"numerator": rate[0], "denominator": rate[1]}, "value": value}


def ids(count: int) -> list[str]:
    """`count` fresh UUIDs, as the model's identifiers."""
    return [str(uuid.uuid4()) for _ in range(count)]


def build_project(
    media: Path,
    out: Path,
    name: str,
    sequence_name: str,
    seconds: float,
    gst_prefix: str | None,
) -> dict[str, Any]:
    """Writes a one-clip project around `media` and reports what it built.

    The project file has to sit above its media - a `MediaPath` is
    project-relative and refuses `..` and absolute paths - so the media is
    linked (or, where a link is not allowed, copied) into `<out>/media/` and
    the `.sub` is written beside it.

    The sequence takes the *source's* canvas and frame rate, not the preset's:
    that is what `settings_for_sequence` does, so a 4K60 source renders 4K60
    whatever preset is chosen, and the preset's own size and rate come back as
    warnings. Both a video and an audio track carry the clip, so every export
    off this project exercises the decoder, the mixer and the audio encoder.
    """
    probed = probe(media, gst_prefix)
    if not probed.ok or not probed.video:
        raise SystemExit(f"{media} is not a video file the discoverer can read: {probed.error}")
    video = probed.video[0]
    rate = video.rate or (30, 1)
    width = video.width or 1920
    height = video.height or 1080

    out.mkdir(parents=True, exist_ok=True)
    media_dir = out / "media"
    media_dir.mkdir(exist_ok=True)
    linked = media_dir / media.name
    if not linked.exists():
        try:
            os.symlink(media.resolve(), linked)
        except (OSError, NotImplementedError, AttributeError):
            try:
                os.link(media.resolve(), linked)
            except OSError:
                shutil.copy2(media, linked)

    frames = max(1, int(round(seconds * rate[0] / rate[1])))
    source = probed.seconds
    if source is not None:
        frames = min(frames, max(1, int(source * rate[0] / rate[1]) - 1))

    media_id, clip_video, clip_audio, video_track, audio_track, sequence_id, project_id, bin_id = ids(8)
    clip = {
        "fade_in": time_value(0, rate),
        "fade_out": time_value(0, rate),
        "gain": 0,
        "id": clip_video,
        "markers": [],
        "media": media_id,
        "name": media.stem,
        "opacity": 1_000_000,
        "source_range": {
            "duration": time_value(frames, rate),
            "start": time_value(0, rate),
        },
        "transform": {
            "position": {"x": 0, "y": 0},
            "rotation_degrees": 0,
            "scale": {"x": 1_000_000, "y": 1_000_000},
        },
    }
    audio_clip = dict(clip, id=clip_audio)

    def track(track_id: str, kind: str, label: str, item: dict[str, Any]) -> dict[str, Any]:
        return {
            "gain": 0,
            "id": track_id,
            "items": [{"clip": item}],
            "kind": kind,
            "locked": False,
            "muted": False,
            "name": label,
            "solo": False,
        }

    # `info` is filled in rather than left null on purpose: `decode_audio`
    # routes a clip through GStreamer when its media item says it has video and
    # through symphonia when it does not, and an item with no info at all takes
    # the second path - which refused the Opus audio of a Matroska source
    # outright (TASK-143, yodaddy, run 34683736400; the routing itself is
    # TASK-150).
    info = {
        "audio": [
            {"channels": stream.channels or 2, "sample_rate": stream.sample_rate or 48000}
            for stream in probed.audio
        ],
        "duration": {
            "rate": {"denominator": 1, "numerator": 1_000_000_000},
            "value": int((probed.seconds or 0) * 1_000_000_000),
        },
        "video": [
            {
                "color": {"primaries": "bt709", "space": "rec709", "transfer": "bt709"},
                "frame_rate": {"denominator": rate[1], "numerator": rate[0]},
                "height": height,
                "sample_aspect": {"denominator": 1, "numerator": 1},
                "width": width,
            }
        ],
    }
    document = {
        "project": {
            "id": project_id,
            "media": [
                {
                    "color": {"primaries": "bt709", "space": "rec709", "transfer": "bt709"},
                    "hash": None,
                    "id": media_id,
                    "info": info,
                    "name": media.name,
                    "offline": False,
                    "path": f"media/{media.name}",
                    "proxy": "none",
                }
            ],
            "name": name,
            "root_bin": {"children": [], "id": bin_id, "media": [media_id], "name": name},
            "sequences": [
                {
                    "id": sequence_id,
                    "markers": [],
                    "name": sequence_name,
                    "settings": {
                        "color": {"primaries": "bt709", "space": "rec709", "transfer": "bt709"},
                        "frame_rate": {"numerator": rate[0], "denominator": rate[1]},
                        "resolution": {"height": height, "width": width},
                        "sample_rate": 48000,
                    },
                    "tracks": [
                        track(video_track, "video", "V1", clip),
                        track(audio_track, "audio", "A1", audio_clip),
                    ],
                }
            ],
        },
        "schema_version": 1,
    }
    project_file = out / f"{sequence_name.lower().replace(' ', '-')}.sub"
    project_file.write_text(json.dumps(document, indent=2, sort_keys=True), encoding="utf-8")
    built = {
        "project": str(project_file),
        "sequence": sequence_name,
        "media": str(linked),
        "width": width,
        "height": height,
        "frame_rate": f"{rate[0]}/{rate[1]}",
        "frames": frames,
        "source_seconds": probed.seconds,
        "source_audio_streams": len(probed.audio),
        "source_audio": [asdict(s) for s in probed.audio],
    }
    # A sidecar beside the project, so `run` can report how many audio streams
    # the source really carried without probing it a second time.
    (out / "source.json").write_text(json.dumps(built, indent=2), encoding="utf-8")
    return built


# --------------------------------------------------------------------------
# The matrix
# --------------------------------------------------------------------------


@dataclass
class Source:
    """One project the matrix renders."""

    name: str
    project: Path
    sequence: str | None
    frames: int | None
    label: str = ""
    audio_streams: int | None = None


@dataclass
class Case:
    """One cell of the matrix."""

    source: str
    preset: str
    encoder: str
    codec: str
    status: str = "pending"
    detail: str = ""
    size: int | None = None
    frames_requested: int | None = None
    frames_expected: int | None = None
    frames_counted: int | None = None
    seconds: float | None = None
    duration: float | None = None
    video_codec: str | None = None
    audio_codec: str | None = None
    audio_channels: int | None = None
    resolution: str | None = None
    output: str | None = None


def render_case(
    cli: str,
    source: Source,
    preset: str,
    encoder: str,
    codec: str,
    extension: str,
    out_dir: Path,
    gst_prefix: str,
    timeout: int,
    env: dict[str, str] | None,
    frames: int | None,
) -> tuple[Case, dict[str, Any], str]:
    """Renders one cell and validates what it wrote."""
    case = Case(source=source.name, preset=preset, encoder=encoder, codec=codec)
    case.frames_requested = frames
    stem = f"{source.name}-{preset}-{encoder}"
    output = out_dir / "outputs" / f"{stem}.{extension}"
    output.parent.mkdir(parents=True, exist_ok=True)
    argv = [
        cli,
        "render",
        str(source.project),
        "--preset",
        preset,
        "--encoder",
        encoder,
        "--out",
        str(output),
        "--verify",
        "--compact",
    ]
    if source.sequence:
        argv += ["--sequence", source.sequence]
    if frames:
        argv += ["--range", f"0:{frames}"]

    done = run(argv, timeout=timeout, env=env)
    case.seconds = round(done.seconds, 1)
    report: dict[str, Any] = {}
    if done.out.strip():
        try:
            report = json.loads(done.out)
        except json.JSONDecodeError:
            report = {}
    # The CLI names the file it wrote: the preset chooses the container, so the
    # extension is not ours to guess.
    written = Path(report.get("path", "")) if report.get("path") else None
    if written is None or not written.exists():
        candidates = sorted(output.parent.glob(stem + "*"))
        written = candidates[0] if candidates else None

    if not done.ok:
        error = (done.err or done.out).strip()
        case.status = "fail"
        case.detail = one_line(error)
        if "export.unknown_encoder" in error:
            case.status = "skip"
            case.detail = "this build's catalogue does not carry the element"
        if "core.not_found" in error and "preset" in error:
            case.status = "skip"
            case.detail = "this build does not ship the preset"
        return case, {"argv": argv, "report": report, "stderr": error[-4000:]}, ""

    case.output = str(written) if written else None
    case.size = written.stat().st_size if written and written.exists() else None
    case.frames_expected = int(report.get("video_frames") or 0) or frames
    settings = report.get("settings") or {}
    case.resolution = (
        f"{settings.get('width')}x{settings.get('height')}" if settings.get("width") else None
    )

    if not written or not written.exists():
        case.status = "fail"
        case.detail = "the render reported success but wrote no file"
        return case, {"argv": argv, "report": report, "stderr": done.err[-4000:]}, ""

    info = probe(written, gst_prefix, timeout=timeout)
    problems: list[str] = []
    if not info.ok:
        problems.append(f"the discoverer refused the file: {one_line(info.error or '')}")
    if not info.video:
        problems.append("no video stream")
    else:
        case.video_codec = info.video[0].codec
        pattern = CODEC_PATTERNS.get(codec)
        if pattern and not pattern.search(info.video[0].codec):
            problems.append(f"the video stream is {info.video[0].codec}, not {codec}")
        if settings.get("width") and info.video[0].width not in (None, settings["width"]):
            problems.append(
                f"the picture is {info.video[0].width}x{info.video[0].height},"
                f" not {settings['width']}x{settings['height']}"
            )
    expected_audio = bool((report.get("settings") or {}).get("audio_codec"))
    if info.audio:
        case.audio_codec = info.audio[0].codec
        case.audio_channels = info.audio[0].channels
    if expected_audio and not info.audio:
        problems.append("the preset asks for audio and the file carries none")
    case.duration = round(info.seconds, 3) if info.seconds is not None else None

    counted, how = count_frames(written, gst_prefix, timeout=timeout)
    case.frames_counted = counted
    if counted is None:
        problems.append(how)
    elif case.frames_expected and counted != case.frames_expected:
        # One frame either way is a muxer's business (a trailing frame the
        # parser holds back); anything more is a dropped picture.
        if abs(counted - case.frames_expected) > 1:
            problems.append(
                f"{counted} frames decoded, {case.frames_expected} written by the encoder"
            )
    if case.frames_expected and info.seconds:
        rate = settings.get("frame_rate") or {}
        numerator = rate.get("numerator")
        denominator = rate.get("denominator") or 1
        if numerator:
            wanted = case.frames_expected * denominator / numerator
            if abs(info.seconds - wanted) > max(0.25, wanted * 0.05):
                problems.append(
                    f"the file lasts {info.seconds:.3f}s, the frames say {wanted:.3f}s"
                )

    case.status = "fail" if problems else "pass"
    case.detail = "; ".join(problems)
    artifacts = {
        "argv": argv,
        "report": report,
        "discoverer": info.text,
        "frame_count": {"frames": counted, "how": how},
        "stderr": done.err[-4000:],
    }
    return case, artifacts, str(written) if written else ""


def frames_for(source: Source, element: str, args: argparse.Namespace) -> int | None:
    """How many frames this cell renders.

    Every cell renders the source's own range except the ones whose encoder is
    slow enough to outlast the job on its own; see `SLOW_ENCODERS`.
    """
    if element in set(args.slow_encoders) and source.frames:
        return min(source.frames, args.slow_frames)
    return source.frames


def cell(text: str, limit: int = 400) -> str:
    """One table cell: no newlines, no pipes, nothing that breaks a row.

    GStreamer's failures are several lines of caps, and a raw one dropped into
    a Markdown table silently ends the table (run 34672568992).
    """
    squashed = " ".join((text or "").split())
    return squashed.replace("|", "/")[:limit]


def one_line(text: str) -> str:
    """A multi-line failure squeezed into a table cell.

    A `SubError` is JSON, so the message and the code are lifted out of it when
    it is one; otherwise the last non-empty line is the closest thing to a
    reason GStreamer gives.
    """
    text = (text or "").strip()
    if not text:
        return "no output"
    for line in reversed(text.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            try:
                document = json.loads(line)
            except json.JSONDecodeError:
                continue
            # A SubError prints as {code, message, details, cause}: the code
            # and message say what the exporter refused, and `cause` carries
            # the flattened GStreamer error underneath it, which is the half a
            # bug report needs.
            error = document.get("error", document)
            code = error.get("code", "")
            message = error.get("message", "")
            details = error.get("details", {}) or {}
            reason = details.get("reason") or details.get("detail") or ""
            cause = error.get("cause") or ""
            joined = " ".join(part for part in (code, message, reason, cause) if part)
            if joined:
                return cell(joined)
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    return cell(lines[-1] if lines else "no output")


def matrix(args: argparse.Namespace) -> int:
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    (out / "failures").mkdir(exist_ok=True)

    discovery = discover(args.cli, args.gst_prefix)
    (out / "discovery.json").write_text(json.dumps(discovery, indent=2), encoding="utf-8")

    sources: list[Source] = []
    for spec in args.source:
        parts = spec.split("::")
        name, project = parts[0], Path(parts[1])
        sequence = parts[2] if len(parts) > 2 and parts[2] else None
        frames = int(parts[3]) if len(parts) > 3 and parts[3] else None
        source = Source(name=name, project=project, sequence=sequence, frames=frames)
        sidecar = project.parent / "source.json"
        if sidecar.exists():
            try:
                built = json.loads(sidecar.read_text(encoding="utf-8"))
                source.audio_streams = built.get("source_audio_streams")
                source.label = f"{built.get('width')}x{built.get('height')} at {built.get('frame_rate')} fps"
            except (json.JSONDecodeError, OSError):
                pass
        sources.append(source)

    presets: dict[str, tuple[str, str]] = dict(DEFAULT_PRESETS)
    if args.presets:
        presets = {name: DEFAULT_PRESETS.get(name, ("h264", "mp4")) for name in args.presets}

    available = pinnable(discovery)
    if args.only_encoders:
        wanted = set(args.only_encoders)
        available = [e for e in available if e["element"] in wanted]
    skip_software_on = set(args.no_software_for)

    cases: list[Case] = []
    env = {"GST_DEBUG": args.gst_debug} if args.gst_debug else None
    for source in sources:
        for encoder in available:
            element = encoder["element"]
            for preset, (codec, extension) in presets.items():
                if codec != encoder["codec"]:
                    continue
                if source.name in skip_software_on and encoder.get("vendor") == "software":
                    cases.append(
                        Case(
                            source=source.name,
                            preset=preset,
                            encoder=element,
                            codec=codec,
                            status="skip",
                            detail="software encoding of this source is measured on the "
                            "hosted and box jobs, not on a paid GPU instance",
                        )
                    )
                    continue
                print(f"---- {source.name} / {preset} / {element}", flush=True)
                started = time.monotonic()
                case, artifacts, written = render_case(
                    args.cli,
                    source,
                    preset,
                    element,
                    codec,
                    extension,
                    out,
                    args.gst_prefix,
                    args.timeout,
                    env,
                    frames_for(source, element, args),
                )
                cases.append(case)
                stem = f"{source.name}-{preset}-{element}"
                print(
                    f"{case.status.upper():5} {stem}"
                    f" [{time.monotonic() - started:.0f}s]: {case.detail or 'ok'}",
                    flush=True,
                )
                if case.status == "fail":
                    keep = out / "failures" / stem
                    keep.mkdir(parents=True, exist_ok=True)
                    (keep / "case.json").write_text(
                        json.dumps({"case": asdict(case), **artifacts}, indent=2, default=str),
                        encoding="utf-8",
                    )
                    if written and Path(written).exists():
                        size = Path(written).stat().st_size
                        if size <= args.keep_bytes:
                            shutil.move(written, keep / Path(written).name)
                        else:
                            (keep / "output.txt").write_text(
                                f"the output was {size} bytes, over the {args.keep_bytes}"
                                " byte artifact limit, and was not kept\n",
                                encoding="utf-8",
                            )
                elif written and Path(written).exists():
                    # A file that passed has nothing left to say and a 4K60
                    # matrix writes gigabytes of them.
                    Path(written).unlink()

    results = {
        "machine": args.machine,
        "discovery": discovery,
        "sources": [asdict(source) | {"project": str(source.project)} for source in sources],
        "presets": {name: {"codec": codec, "container": extension}
                    for name, (codec, extension) in presets.items()},
        "cases": [asdict(case) for case in cases],
    }
    (out / "results.json").write_text(json.dumps(results, indent=2, default=str), encoding="utf-8")
    summary = table(args.machine, discovery, sources, cases)
    (out / "summary.md").write_text(summary, encoding="utf-8")
    print(summary)

    failures = [case for case in cases if case.status == "fail"]
    for case in failures:
        print(
            f"::error::{args.machine}: {case.encoder} + {case.preset} on {case.source}"
            f" failed: {case.detail}",
            flush=True,
        )
    if failures and args.fail_on_error:
        return 1
    return 0


def table(
    machine: str,
    discovery: dict[str, Any],
    sources: list[Source],
    cases: list[Case],
) -> str:
    """The job summary: the discovery, then the matrix, then the notes."""
    lines: list[str] = []
    lines.append(f"## Export matrix - {machine}")
    lines.append("")
    lines.append(f"GStreamer: `{discovery.get('gstreamer') or 'unknown'}`  ")
    lines.append(f"Host: `{discovery.get('platform')}`")
    lines.append("")
    lines.append("### Encoders this machine carries")
    lines.append("")
    lines.append("| element | codec | in registry | probe | note |")
    lines.append("| --- | --- | --- | --- | --- |")
    for encoder in discovery["encoders"]:
        if not encoder["in_registry"] and not encoder["present"]:
            continue
        if encoder["ready"]:
            state = "ready, ranked NONE (pinned only)" if encoder["deranked"] else "ready"
        elif encoder["present"]:
            state = "not ready"
        else:
            state = "missing"
        note = cell(encoder.get("detail") or "", 120)
        if not encoder["catalogued"]:
            note = (note + " " if note else "") + "(not in this build's catalogue)"
        lines.append(
            f"| `{encoder['element']}` | {encoder['codec']} |"
            f" {'yes' if encoder['in_registry'] else 'no'} | {state} | {note} |"
        )
    lines.append("")
    lines.append("### machine x source x preset x encoder")
    lines.append("")
    lines.append(
        "| machine | source | preset | encoder | result | size | frames (decoded/written)"
        " | duration | video | audio | detail |"
    )
    lines.append("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    mark = {"pass": "pass", "fail": "**FAIL**", "skip": "skip"}
    by_name = {source.name: source for source in sources}
    for case in cases:
        size = f"{case.size / 1_000_000:.1f} MB" if case.size else "-"
        frames = (
            f"{case.frames_counted if case.frames_counted is not None else '?'}"
            f"/{case.frames_expected if case.frames_expected is not None else '?'}"
        )
        source = by_name.get(case.source)
        if (
            case.frames_requested
            and source
            and source.frames
            and case.frames_requested != source.frames
        ):
            frames += " (short range)"

        duration = f"{case.duration:.2f}s" if case.duration else "-"
        audio = (
            f"{case.audio_codec} {case.audio_channels}ch"
            if case.audio_codec
            else ("none" if case.status == "pass" else "-")
        )
        lines.append(
            f"| {machine} | {case.source} | {case.preset} | `{case.encoder}` |"
            f" {mark.get(case.status, case.status)} | {size} | {frames} | {duration} |"
            f" {case.video_codec or '-'} | {audio} | {cell(case.detail)} |"
        )
    lines.append("")
    passed = sum(1 for case in cases if case.status == "pass")
    failed = sum(1 for case in cases if case.status == "fail")
    skipped = sum(1 for case in cases if case.status == "skip")
    lines.append(f"**{passed} passed, {failed} failed, {skipped} skipped.**")
    lines.append("")
    lines.append("### Notes")
    lines.append("")
    lines.append(
        "- Frames are counted out of the written file, not taken from the"
        " encoder's own count: the left number is what came back out, read off"
        " the demuxed video track (or, for a container with no demuxer entry,"
        " a full decode)."
    )
    lines.append(
        "- The canvas and the frame rate come from the *sequence*, not from the"
        " preset (`settings_for_sequence`), so a 4K60 source is written 4K60"
        " under a 1080p30 preset and the preset's own size and rate are"
        " reported as warnings."
    )
    lines.append(
        "- The `audio-only` preset is not in the matrix: it carries no video"
        " stream, and `render` refuses it before any encoder is chosen."
    )
    slow = sorted(
        {
            case.encoder
            for case in cases
            if case.frames_requested
            and by_name.get(case.source)
            and by_name[case.source].frames
            and case.frames_requested != by_name[case.source].frames
        }
    )
    if slow:
        lines.append(
            "- " + ", ".join(f"`{element}`" for element in slow) + " render a short"
            " range: software AV1 is seconds to minutes a frame at stock settings,"
            " and a full-length cell would outlast the job."
        )
    for source in sources:
        if source.audio_streams:
            lines.append(
                f"- `{source.name}` carries {source.audio_streams} audio streams;"
                " the exporter mixes the first one (uridecodebin exposes one"
                " audio pad) and writes a single stereo track."
            )
    return "\n".join(lines) + "\n"


# --------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--cli", default="subordinate-cli", help="the subordinate-cli to run")
    common.add_argument(
        "--gst-prefix",
        default="",
        help="prefix for the GStreamer tools, e.g. 'subordinate-' on the desktop image",
    )

    run_parser = sub.add_parser("run", parents=[common], help="run the matrix")
    run_parser.add_argument("--machine", required=True)
    run_parser.add_argument("--out", required=True)
    run_parser.add_argument(
        "--source",
        action="append",
        default=[],
        metavar="NAME::PROJECT[::SEQUENCE[::FRAMES]]",
        help="a project to render, its sequence and how many frames of it",
    )
    # Workflow filters travel as environment data, never interpolated shell code.
    # Explicit CLI flags retain precedence; empty filters keep the full matrix.
    run_parser.add_argument(
        "--presets", nargs="*", default=os.environ.get("MATRIX_PRESETS", "").split() or None
    )
    run_parser.add_argument(
        "--only-encoders", nargs="*", default=os.environ.get("MATRIX_ENCODERS", "").split() or None
    )
    run_parser.add_argument(
        "--no-software-for",
        nargs="*",
        default=[],
        help="sources whose software-encoder cells are skipped on this machine",
    )
    run_parser.add_argument(
        "--timeout",
        type=int,
        default=600,
        help="seconds one render, probe or frame count may take before the cell is failed",
    )
    run_parser.add_argument("--keep-bytes", type=int, default=200_000_000)
    run_parser.add_argument("--gst-debug", default=None)
    run_parser.add_argument("--slow-encoders", nargs="*", default=list(SLOW_ENCODERS))
    run_parser.add_argument("--slow-frames", type=int, default=8)
    run_parser.add_argument("--fail-on-error", action="store_true")

    project_parser = sub.add_parser(
        "project", parents=[common], help="build a .sub around one media file"
    )
    project_parser.add_argument("--media", required=True)
    project_parser.add_argument("--out", required=True)
    project_parser.add_argument("--name", default="Export matrix")
    project_parser.add_argument("--sequence", default="Matrix")
    project_parser.add_argument("--seconds", type=float, default=4.0)

    discover_parser = sub.add_parser(
        "discover", parents=[common], help="print the encoder discovery as JSON"
    )
    discover_parser.add_argument("--out", default=None)

    args = parser.parse_args()
    args.gst_prefix = args.gst_prefix or None

    if args.command == "run":
        return matrix(args)
    if args.command == "project":
        built = build_project(
            Path(args.media),
            Path(args.out),
            args.name,
            args.sequence,
            args.seconds,
            args.gst_prefix,
        )
        print(json.dumps(built, indent=2))
        return 0
    if args.command == "discover":
        found = discover(args.cli, args.gst_prefix)
        text = json.dumps(found, indent=2)
        if args.out:
            Path(args.out).write_text(text, encoding="utf-8")
        print(text)
        return 0
    return 2


if __name__ == "__main__":
    sys.exit(main())
