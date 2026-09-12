#!/usr/bin/env python3
"""Run relocated first-use regressions without opening or controlling a desktop."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import wave

artifact = Path(sys.argv[1]).resolve()
manifest = json.loads((artifact / "build-info.json").read_text(encoding="utf-8"))
expected_sha = os.environ.get("SUB_REGRESSION_ARTIFACT_SHA") or os.environ["GITHUB_SHA"]
if manifest["sha"] != expected_sha:
    raise SystemExit("artifact commit differs from the requested build")
print(f"Harness commit: {os.environ['GITHUB_SHA']}; binary commit: {expected_sha}", flush=True)
for entry in manifest["executables"].values():
    path = artifact / entry["file"]
    if hashlib.sha256(path.read_bytes()).hexdigest() != entry["sha256"]:
        raise SystemExit(f"binary hash mismatch: {path}")
    if os.name != "nt":
        path.chmod(path.stat().st_mode | 0o111)
with tempfile.TemporaryDirectory(prefix="sub-reg-") as scratch:
    env = dict(os.environ)
    for key in ("XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "APPDATA", "LOCALAPPDATA", "SUBORDINATE_ENDPOINT_DIR"):
        env[key] = str(Path(scratch) / key.lower())
        Path(env[key]).mkdir()
    env["SUBORDINATE_REPO_ROOT"] = str(Path.cwd())
    env["SUBORDINATE_INSTANCE"] = "fresh-regression"
    env["GST_REGISTRY"] = str(Path(scratch) / "gst-registry.bin")
    fixtures = Path(scratch) / "fixtures"
    fixtures.mkdir()
    env["SUB_FIXTURES_DIR"] = str(fixtures)
    with wave.open(str(fixtures / "tone_48k_stereo.wav"), "wb") as wav:
        wav.setnchannels(2)
        wav.setsampwidth(2)
        wav.setframerate(48000)
        wav.writeframes(bytes(48000 * 2 * 2))
    gst_launch = str(artifact / "runtime" / "bin" / "gst-launch-1.0.exe") if os.name == "nt" else "gst-launch-1.0"
    subprocess.run([gst_launch, "-q", "-e", "videotestsrc", "num-buffers=125", "!",
        "video/x-raw,width=1920,height=1080,framerate=25/1", "!", "x264enc", "speed-preset=ultrafast",
        "!", "h264parse", "!", "mp4mux", "!", "filesink", "location=" + str(fixtures / "bars_1080p_h264.mp4")],
        env=env, check=True, timeout=90)
    entries = []
    for name, kind, width, height, seconds, fps in [
            ("bars_1080p_h264.mp4", "video", 1920, 1080, 5, 25),
            ("tone_48k_stereo.wav", "audio", 0, 0, 1, 0)]:
        entries.append({"name": name, "kind": kind, "width": width, "height": height,
            "duration_ns": seconds * 1000000000, "fps_num": fps, "fps_den": 1,
            "vfr": False, "lossy": kind == "video", "generated": True,
            "description": "minimal first-use regression fixture"})
    (fixtures / "manifest.json").write_text(json.dumps({"version": 1,
        "generator": "scripts/desktop/run-regressions.py", "fixtures": entries}), encoding="utf-8")
    env["SUBORDINATE_MCP"] = str(artifact / manifest["executables"]["subordinate-mcp"]["file"])
    env["SUBORDINATE_CLI"] = str(artifact / manifest["executables"]["subordinate-cli"]["file"])
    # The MCP acceptance test serves its own temporary endpoint. No session
    # discovery, key injection, window launch or user configuration mutation.
    tests = [("empty_timeline_drop", []), ("empty_track_menu", []), ("media_import_app", ["unsaved_import_preview_and_first_save_keep_sources_and_pending_jobs", "--exact"]),
             ("stdio", ["unsaved_external_import_survives_undo_save_and_reopen_over_stdio", "--exact"])]
    for name, select in tests:
        command = [str(artifact / manifest["executables"][name]["file"]), *select]
        listed = subprocess.run([*command, "--list"], env=env, text=True, capture_output=True, timeout=60)
        if listed.returncode or not any(line.endswith(": test") for line in listed.stdout.splitlines()):
            raise SystemExit(f"no matching regression tests in {name}: {listed.stdout}{listed.stderr}")
        result = subprocess.run([*command, "--nocapture", "--test-threads=1"], env=env,
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=300)
        print(result.stdout, flush=True)
        if result.returncode or "skipping:" in result.stdout.lower():
            raise SystemExit(f"{name} failed or skipped (exit {result.returncode})")
print(f"UI and MCP first-use regressions passed on {manifest['sha']}")
