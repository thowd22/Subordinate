#!/usr/bin/env python3
"""Turn a hardware-verification run into a job summary, and gate it on numbers.

`.github/workflows/hardware.yml` runs the benchmark harness
(`bins/subordinate-bench`, see docs/PERFORMANCE.md) and the compositor readback
throughput measurement on a real GPU, then calls this script to render both
into the GitHub job summary and to fail the job when a number is below the
criterion it is there to prove:

* 4K H.264 scrub above 30 fps *with a hardware decoder* (docs/PLAN.md §8).
  The rate itself is reported as a warning rather than a failure: it is a
  measurement of the machine, and a number below the plan's target is work for
  the verify tasks, not a broken workflow. That the decode went through a
  hardware decoder at all *is* a failure when it did not, because a software
  decode makes the number meaningless;
* compositor readback above 60 fps on a 1080p canvas (TASK-58 AC 2, moved to
  TASK-116), asserted on the NVIDIA job only.

Python, not jq: python3 is on every Ubuntu runner including the self-hosted
box, jq is not guaranteed. Rates in the report are exact integers in
milli-frames per second (`30_500` is 30.5 fps) and are kept that way here;
only the printed text divides by a thousand.

Usage:
  hardware-summary.py --label NAME --perf FILE [--readback FILE]
                      [--min-scrub-fps N] [--min-readback-fps N]
                      [--require-hardware-decode PREFIX[,PREFIX...]]
                      [--out FILE]

Exit status is 0 when every gate that was asked for passed, 1 otherwise. A
gate left at 0 (the default) is reported but never fails the job.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# The fixture the 4K scrub criterion is stated against (scripts/gen-fixtures.sh).
SCRUB_FIXTURE = "bars_2160p_h264.mp4"

# "1080p readback: 60 frames in 412.3ms = 145.5 fps on NVIDIA T4 (Vulkan)"
READBACK_LINE = re.compile(r"1080p readback:.*?=\s*([0-9]+(?:\.[0-9]+)?)\s*fps\s*on\s*(.*)")


def milli_fps(value):
    """Format a milli-fps integer as fps, or `n/a` when it is missing."""
    if value is None:
        return "n/a"
    return f"{value // 1000}.{value % 1000:03d}"


def parse_args(argv):
    parser = argparse.ArgumentParser(add_help=True)
    parser.add_argument("--label", required=True, help="runner name for the heading")
    parser.add_argument("--perf", required=True, type=Path, help="perf.json from subordinate-bench")
    parser.add_argument("--readback", type=Path, help="log of the readback throughput test")
    parser.add_argument("--min-scrub-fps", type=float, default=0.0)
    parser.add_argument(
        "--scrub-severity",
        choices=["error", "warning"],
        default="error",
        help="whether a 4K scrub below --min-scrub-fps fails the job (default) or only warns",
    )
    parser.add_argument("--min-readback-fps", type=float, default=0.0)
    parser.add_argument(
        "--require-hardware-decode",
        default="",
        help="comma-separated element name prefixes one of which the 4K decoder must match",
    )
    parser.add_argument("--out", type=Path, help="write the markdown here as well as to stdout")
    return parser.parse_args(argv)


def main(argv):
    args = parse_args(argv)
    lines: list[str] = [f"### Hardware verification - {args.label}", ""]
    failures: list[str] = []
    warnings: list[str] = []

    if not args.perf.is_file():
        failures.append(f"no benchmark report at {args.perf}")
        report = None
    else:
        report = json.loads(args.perf.read_text(encoding="utf-8"))

    if report is not None:
        host = report.get("host", {})
        gpu = report.get("gpu")
        adapter = "none (the texture stage was skipped)"
        if gpu:
            adapter = gpu.get("adapter", "unknown")
            if gpu.get("software"):
                adapter += " **[software adapter: not a hardware measurement]**"
        lines += [
            f"* host: `{host.get('os')}` `{host.get('arch')}`, {host.get('profile')} build",
            f"* adapter: {adapter}",
            "",
            "| Scenario | Fixture | Size | Decoder | Sustained fps | p50 | p95 |",
            "| --- | --- | --- | --- | --- | --- | --- |",
        ]
        scrub = None
        for scenario in report.get("scenarios", []):
            if scenario.get("status") != "measured":
                lines.append(
                    f"| {scenario.get('kind')} | `{scenario.get('fixture')}` | - | - | "
                    f"skipped: {scenario.get('skipped_reason', 'no reason given')} | - | - |"
                )
                continue
            latency = scenario.get("decode_to_texture") or scenario.get("decode") or {}
            lines.append(
                "| {kind} | `{fixture}` | {w}x{h} | `{dec}` | {fps} | {p50} | {p95} |".format(
                    kind=scenario.get("kind"),
                    fixture=scenario.get("fixture"),
                    w=scenario.get("width"),
                    h=scenario.get("height"),
                    dec=scenario.get("decoder") or "unknown",
                    fps=milli_fps(scenario.get("sustained_milli_fps")),
                    p50=f"{latency.get('p50_nanos', 0) / 1e6:.3f} ms",
                    p95=f"{latency.get('p95_nanos', 0) / 1e6:.3f} ms",
                )
            )
            if scenario.get("kind") == "scrub" and scenario.get("fixture") == SCRUB_FIXTURE:
                scrub = scenario
        lines.append("")

        if scrub is None:
            failures.append(f"the {SCRUB_FIXTURE} scrub scenario did not produce numbers")
        else:
            fps = (scrub.get("sustained_milli_fps") or 0) / 1000.0
            decoder = scrub.get("decoder") or "unknown"
            # Where a scrub step's time actually went. A step is a flushing
            # keyframe seek plus the decode-forward from that keyframe to the
            # frame asked for, and the two answer to different fixes, so the
            # summary says which half the gap to the criterion lives in
            # (TASK-133, docs/PERFORMANCE.md).
            seek = scrub.get("seek")
            forward = scrub.get("decode_forward")
            if seek and forward:
                seeks = scrub.get("seeks_issued") or 0
                frames = scrub.get("frames_decoded") or 0
                per_seek = f"{frames / seeks:.1f}" if seeks else "n/a"
                lines.append(
                    f"* 4K scrub step split: seek p50 **{seek.get('p50_nanos', 0) / 1e6:.3f} ms**, "
                    f"decode-forward p50 **{forward.get('p50_nanos', 0) / 1e6:.3f} ms**, "
                    f"{per_seek} pictures decoded per seek"
                )
            if args.min_scrub_fps > 0:
                verdict = "PASS" if fps > args.min_scrub_fps else "FAIL"
                lines.append(
                    f"* 4K H.264 scrub: **{fps:.3f} fps** through `{decoder}` "
                    f"(criterion: above {args.min_scrub_fps:g} fps) - {verdict}"
                )
                if verdict == "FAIL":
                    message = f"4K scrub is {fps:.3f} fps, below {args.min_scrub_fps:g}"
                    if args.scrub_severity == "error":
                        failures.append(message)
                    else:
                        warnings.append(message)
            wanted = [p for p in args.require_hardware_decode.split(",") if p]
            if wanted:
                ok = any(decoder.startswith(prefix) for prefix in wanted)
                lines.append(
                    f"* 4K decode element: `{decoder}` "
                    f"(hardware decoders here: {', '.join(wanted)}) - {'PASS' if ok else 'FAIL'}"
                )
                if not ok:
                    failures.append(
                        f"the 4K scrub decoded with `{decoder}`, not one of {', '.join(wanted)}"
                    )

    if args.readback:
        text = args.readback.read_text(encoding="utf-8") if args.readback.is_file() else ""
        match = READBACK_LINE.search(text)
        if match is None:
            lines.append("* compositor readback (1080p): **not measured** (no adapter, or the test did not run)")
            if args.min_readback_fps > 0:
                failures.append("the compositor readback throughput test produced no number")
        else:
            fps = float(match.group(1))
            note = ""
            if args.min_readback_fps > 0:
                verdict = "PASS" if fps > args.min_readback_fps else "FAIL"
                note = f" (criterion: above {args.min_readback_fps:g} fps) - {verdict}"
                if verdict == "FAIL":
                    failures.append(f"readback is {fps:.1f} fps, below {args.min_readback_fps:g}")
            lines.append(
                f"* compositor readback, 1080p canvas: **{fps:.1f} fps** on {match.group(2).strip()}{note}"
            )

    lines.append("")
    if warnings:
        lines.append("**Below target, reported only**")
        lines += [f"* {reason}" for reason in warnings]
        lines.append("")
    if failures:
        lines.append("**Failed checks**")
        lines += [f"* {reason}" for reason in failures]

    markdown = "\n".join(lines) + "\n"
    sys.stdout.write(markdown)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(markdown, encoding="utf-8")

    for reason in warnings:
        print(f"::warning::{reason}", file=sys.stderr)
    for reason in failures:
        print(f"::error::{reason}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
