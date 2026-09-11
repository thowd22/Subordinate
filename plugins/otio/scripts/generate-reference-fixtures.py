#!/usr/bin/env python3
"""Regenerate the OTIO fixtures that the reference implementation produced.

`otio-core/tests/reference.rs` imports files written by the *reference*
OpenTimelineIO Python library rather than by this plugin, because a round trip
through one implementation proves only that it agrees with itself. This script
is how those fixtures were made, and running it again reproduces them byte for
byte.

Three fixtures come out of it:

* `reference_multitrack.otio` - one of the timelines shipped in the reference
  repository's `tests/sample_data`, read and written back by the library's own
  `otio_json` adapter, so the file carries the schema versions and the field
  ordering that library emits.
* `reference_nucoda_edl.otio` - a CMX 3600 EDL converted by the `cmx_3600`
  adapter. Its events name their media with `* FROM FILE`, so the clips come
  out carrying `ExternalReference`s with Windows paths.
* `reference_screening_edl.otio` - a second, longer EDL conversion whose events
  name no file at all, so every clip comes out with a `MissingReference` and
  three carry `* LOC` markers. It is what an EDL out of a cutting room usually
  looks like.

Requirements (the versions the committed fixtures were generated with):

    python3 -m pip install opentimelineio==0.18.1 otio-cmx3600-adapter==1.0.0

The source files are downloaded from the pinned tags rather than vendored, so
that what is committed here is only the reference library's output.

Usage:

    python3 plugins/otio/scripts/generate-reference-fixtures.py [--out DIR]
    python3 plugins/otio/scripts/generate-reference-fixtures.py --check
"""

from __future__ import annotations

import argparse
import pathlib
import sys
import tempfile
import urllib.request

OTIO_VERSION = "0.18.1"
CMX_VERSION = "1.0.0"

OTIO_SAMPLES = (
    "https://raw.githubusercontent.com/AcademySoftwareFoundation/"
    f"OpenTimelineIO/v{OTIO_VERSION}/tests/sample_data/"
)
CMX_SAMPLES = (
    "https://raw.githubusercontent.com/OpenTimelineIO/"
    f"otio-cmx3600-adapter/v{CMX_VERSION}/tests/sample_data/"
)

# (fixture name, source url, adapter to read it with, rate for EDL reads)
FIXTURES = (
    ("reference_multitrack.otio", OTIO_SAMPLES + "multiple_track.otio", "otio_json", None),
    ("reference_nucoda_edl.otio", CMX_SAMPLES + "nucoda_example.edl", "cmx_3600", 24),
    ("reference_screening_edl.otio", CMX_SAMPLES + "screening_example.edl", "cmx_3600", 24),
)

DEFAULT_OUT = pathlib.Path(__file__).resolve().parent.parent / "otio-core" / "tests" / "fixtures"


def check_versions() -> None:
    """Refuse to write fixtures with a library that would write them differently."""
    import opentimelineio as otio

    if otio.__version__ != OTIO_VERSION:
        sys.exit(
            f"opentimelineio {OTIO_VERSION} is what the fixtures were generated with, "
            f"found {otio.__version__}"
        )
    adapters = [adapter.name for adapter in otio.plugins.ActiveManifest().adapters]
    if "cmx_3600" not in adapters:
        sys.exit("the cmx_3600 adapter is missing: pip install otio-cmx3600-adapter")


def generate(url: str, adapter: str, rate: int | None) -> str:
    """Read one source file with the reference library and serialise it as OTIO JSON."""
    import opentimelineio as otio

    suffix = pathlib.PurePosixPath(url).suffix
    with tempfile.NamedTemporaryFile(suffix=suffix) as source:
        with urllib.request.urlopen(url) as response:  # noqa: S310 - pinned https urls
            source.write(response.read())
        source.flush()
        if rate is None:
            timeline = otio.adapters.read_from_file(source.name, adapter)
        else:
            timeline = otio.adapters.read_from_file(source.name, adapter, rate=rate)
    # Everything is written back through otio_json, so every fixture is this
    # library's own serialisation whatever it was read from.
    return otio.adapters.write_to_string(timeline, "otio_json") + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description="Regenerate the reference OTIO fixtures.")
    parser.add_argument("--out", type=pathlib.Path, default=DEFAULT_OUT, help="where to write")
    parser.add_argument(
        "--check",
        action="store_true",
        help="regenerate in memory and fail if the committed fixtures differ",
    )
    args = parser.parse_args()

    check_versions()
    stale = []
    for name, url, adapter, rate in FIXTURES:
        document = generate(url, adapter, rate)
        target = args.out / name
        if args.check:
            if not target.exists() or target.read_text(encoding="utf-8") != document:
                stale.append(name)
            continue
        target.write_text(document, encoding="utf-8", newline="\n")
        print(f"wrote {target} ({len(document)} bytes)")
    if stale:
        print("stale fixtures: " + ", ".join(stale), file=sys.stderr)
        return 1
    if args.check:
        print("fixtures match the reference library's output")
    return 0


if __name__ == "__main__":
    sys.exit(main())
