#!/usr/bin/env bash
# Test the packaging data: the things that break a release silently.
#
# Packaging has no Rust code to unit-test, but it does have four files that
# must agree with each other and with Cargo.toml -- the desktop entry, the
# AppStream metainfo, the plugin allowlist and the Flatpak manifest. A typo in
# any of them survives `cargo test` and shows up as an AppImage with no icon, a
# Flatpak that cannot reach the GPU, or an export that silently drops to
# software encode. This script is what CI and a developer run instead.
#
# Usage:
#   packaging/validate.sh [--appdir DIR]
#
#   --appdir DIR   additionally check a staged AppDir (see
#                  packaging/linux/build-appimage.sh --stage-only)
#
# Optional tools are used when present and skipped when not:
# desktop-file-validate, appstreamcli, shellcheck, python3 with PyYAML.
set -uo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
linux_dir=$repo_root/packaging/linux
flatpak_dir=$repo_root/packaging/flatpak
app_id=io.github.thowd22.Subordinate
manifest=$flatpak_dir/$app_id.yml
metainfo=$linux_dir/$app_id.metainfo.xml
desktop=$linux_dir/subordinate.desktop
plugins=$linux_dir/gst-plugins.txt
apprun=$linux_dir/AppRun

appdir=
while [ $# -gt 0 ]; do
    case "$1" in
        --appdir) appdir=${2:?--appdir needs a directory}; shift ;;
        -h|--help) sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "validate.sh: unknown option $1" >&2; exit 2 ;;
    esac
    shift
done

failures=0
skipped=0

fail() { echo "FAIL: $*"; failures=$((failures + 1)); }
pass() { echo "ok:   $*"; }
skip() { echo "skip: $*"; skipped=$((skipped + 1)); }
check() {
    # check <description> <command...>
    local what=$1
    shift
    if "$@" >/dev/null 2>&1; then pass "$what"; else fail "$what"; fi
}

version=$(sed -n '/^\[workspace.package\]/,/^\[/p' "$repo_root/Cargo.toml" |
    sed -n 's/^version = "\(.*\)"/\1/p' | head -n1)
[ -n "$version" ] || { echo "validate.sh: cannot read the workspace version" >&2; exit 1; }
echo "workspace version $version"

# --- the files exist at the paths the scripts install from ------------------
for file in "$manifest" "$metainfo" "$desktop" "$plugins" "$apprun" \
    "$linux_dir/subordinate.svg" "$linux_dir/build-appimage.sh" \
    "$flatpak_dir/build-flatpak.sh"; do
    [ -f "$file" ] && pass "present ${file#"$repo_root/"}" || fail "missing ${file#"$repo_root/"}"
done

# --- shell scripts ----------------------------------------------------------
# AppRun runs on hosts with no bash, so it is checked with POSIX sh.
check "AppRun is valid POSIX sh" sh -n "$apprun"
for script in "$linux_dir/build-appimage.sh" "$flatpak_dir/build-flatpak.sh" \
    "${BASH_SOURCE[0]}"; do
    check "bash -n ${script#"$repo_root/"}" bash -n "$script"
    [ -x "$script" ] || fail "${script#"$repo_root/"} is not executable"
done
[ -x "$apprun" ] || fail "AppRun is not executable"
if command -v shellcheck >/dev/null 2>&1; then
    check "shellcheck packaging scripts" shellcheck -S warning \
        "$apprun" "$linux_dir/build-appimage.sh" "$flatpak_dir/build-flatpak.sh"
else
    skip "shellcheck not installed"
fi

# --- desktop entry ----------------------------------------------------------
desktop_value() { sed -n "s/^$1=//p" "$desktop" | head -n1; }
[ "$(desktop_value Type)" = Application ] || fail "desktop Type is not Application"
[ "$(desktop_value Exec)" = "subordinate %f" ] || fail "desktop Exec is not 'subordinate %f'"
[ "$(desktop_value Icon)" = "$app_id" ] || fail "desktop Icon is not $app_id"
grep -q '^Categories=.*AudioVideo' "$desktop" || fail "desktop Categories lacks AudioVideo"
grep -q '^\[Desktop Entry\]' "$desktop" || fail "desktop entry has no [Desktop Entry] group"
pass "desktop entry keys"
if command -v desktop-file-validate >/dev/null 2>&1; then
    check "desktop-file-validate" desktop-file-validate "$desktop"
else
    skip "desktop-file-validate not installed"
fi

# --- AppStream metainfo -----------------------------------------------------
check "metainfo is well-formed XML" python3 -c \
    "import sys,xml.etree.ElementTree as e; e.parse(sys.argv[1])" "$metainfo"
metainfo_id=$(python3 -c \
    "import sys,xml.etree.ElementTree as e; print(e.parse(sys.argv[1]).findtext('id',''))" \
    "$metainfo" 2>/dev/null)
[ "$metainfo_id" = "$app_id" ] || fail "metainfo id is '$metainfo_id', expected $app_id"
metainfo_version=$(python3 -c \
    "import sys,xml.etree.ElementTree as e
r=e.parse(sys.argv[1]).find('releases/release')
print(r.get('version','') if r is not None else '')" "$metainfo" 2>/dev/null)
if [ "$metainfo_version" = "$version" ]; then
    pass "metainfo release $metainfo_version matches Cargo.toml"
else
    fail "metainfo release is '$metainfo_version', Cargo.toml says $version"
fi
grep -q "<launchable type=\"desktop-id\">$app_id.desktop</launchable>" "$metainfo" ||
    fail "metainfo launchable does not point at $app_id.desktop"
if command -v appstreamcli >/dev/null 2>&1; then
    if appstreamcli validate --no-net "$metainfo" >/tmp/appstream.$$ 2>&1; then
        pass "appstreamcli validate"
    else
        # Warnings and infos are advice; only errors fail the build.
        if grep -qE '^[EF]:' /tmp/appstream.$$; then
            fail "appstreamcli validate"
            sed -n '1,40p' /tmp/appstream.$$
        else
            pass "appstreamcli validate (warnings only)"
            grep -E '^W:' /tmp/appstream.$$ | head -n 10
        fi
    fi
    rm -f /tmp/appstream.$$
else
    skip "appstreamcli not installed"
fi

# --- plugin allowlist -------------------------------------------------------
bad_entries=$(sed 's/#.*//' "$plugins" | tr -d ' \t' | grep -v '^$' |
    grep -vE '^!?[a-z0-9_]+$' || true)
[ -z "$bad_entries" ] || fail "malformed plugin entries: $(echo "$bad_entries" | tr '\n' ' ')"
entries=$(sed 's/#.*//' "$plugins" | tr -d ' \t!' | grep -v '^$')
duplicates=$(echo "$entries" | sort | uniq -d)
[ -z "$duplicates" ] || fail "duplicate plugin entries: $(echo "$duplicates" | tr '\n' ' ')"
# voaacenc rather than libav for AAC: gst-libav registers avenc_aac at rank
# NONE and sub-export will not plug a deranked element.
for must in nvcodec va coreelements app playback libav x264 voaacenc isomp4 matroska; do
    grep -qx "!$must" <(sed 's/#.*//' "$plugins" | tr -d ' \t') ||
        fail "plugin allowlist does not mark $must as required"
done
pass "plugin allowlist ($(echo "$entries" | wc -l) modules, hardware plugins required)"

# --- AppRun -----------------------------------------------------------------
# Without these three the bundled plugins are invisible and the editor falls
# back to whatever the host happens to have, which is the failure this package
# exists to prevent.
for var in GST_PLUGIN_SYSTEM_PATH_1_0 GST_PLUGIN_SCANNER GST_REGISTRY LD_LIBRARY_PATH; do
    grep -q "export .*$var" "$apprun" || fail "AppRun does not export $var"
done
grep -q 'exec "\$here/usr/bin/subordinate"' "$apprun" || fail "AppRun does not exec the editor"
pass "AppRun exports the GStreamer environment"

# --- flatpak manifest -------------------------------------------------------
if python3 -c 'import yaml' 2>/dev/null; then
    python3 - "$manifest" "$app_id" "$repo_root" <<'PY'
import sys, os, yaml

path, app_id, repo_root = sys.argv[1:4]
with open(path, encoding="utf-8") as handle:
    m = yaml.safe_load(handle)

problems = []
if m.get("id") != app_id:
    problems.append(f"id is {m.get('id')!r}, expected {app_id}")
if m.get("runtime") != "org.freedesktop.Platform":
    problems.append("runtime is not org.freedesktop.Platform")
if m.get("command") != "subordinate":
    problems.append("command is not subordinate")

runtime_version = str(m.get("runtime-version", ""))
finish = m.get("finish-args", [])
for arg in ("--device=dri", "--socket=wayland", "--socket=pulseaudio"):
    if arg not in finish:
        problems.append(f"finish-args lacks {arg}")

# The GStreamer story is the reason this manifest exists: the freedesktop
# runtime's own GStreamer plus its extension point, not a bundled copy. Since
# branch 25.08 the runtime also declares org.freedesktop.Platform.codecs-extra
# (the replacement for the retired ffmpeg-full) and mounts it on
# GST_PLUGIN_SYSTEM_PATH itself, so the app must NOT re-declare either one --
# an add-extensions entry with the same name shadows the runtime's mount point
# and the codecs disappear.
if tuple(int(part) for part in runtime_version.split(".")) < (25, 8):
    problems.append(
        f"runtime-version {runtime_version} predates 25.08: its rust-stable SDK "
        "extension is rustc 1.89, below the workspace rust-version"
    )
for retired in ("org.freedesktop.Platform.ffmpeg-full",):
    if retired in (m.get("add-extensions") or {}):
        problems.append(f"{retired} does not exist on runtime {runtime_version}")
for shadowed in ("org.freedesktop.Platform.codecs-extra",
                 "org.freedesktop.Platform.GStreamer"):
    if shadowed in (m.get("add-extensions") or {}):
        problems.append(f"add-extensions re-declares the runtime's own {shadowed}")
if any("ffmpeg" in a for a in finish):
    problems.append("finish-args still reference the retired ffmpeg-full extension")
with open(path, encoding="utf-8") as handle:
    if "org.freedesktop.Platform.GStreamer" not in handle.read():
        problems.append("manifest never mentions the freedesktop GStreamer extension")

modules = m.get("modules", [])
if not modules:
    problems.append("no modules")
else:
    commands = " ".join(modules[0].get("build-commands", []))
    for needed in ("subordinate.desktop", f"{app_id}.metainfo.xml", "subordinate.svg"):
        if needed not in commands:
            problems.append(f"build-commands never install {needed}")
    # Every file the manifest installs has to exist in the tree it copies.
    for token in commands.split():
        if token.startswith("packaging/") and not os.path.exists(
            os.path.join(repo_root, token)
        ):
            problems.append(f"build-commands install a missing file: {token}")
    options = modules[0].get("build-options", {})
    if "/usr/lib/sdk/rust-stable/bin" not in options.get("append-path", ""):
        problems.append("the rust-stable SDK extension is not on PATH")
if "org.freedesktop.Sdk.Extension.rust-stable" not in (m.get("sdk-extensions") or []):
    problems.append("the rust-stable SDK extension is not requested")

for problem in problems:
    print(f"FAIL: flatpak manifest: {problem}")
sys.exit(1 if problems else 0)
PY
    if [ $? -eq 0 ]; then pass "flatpak manifest"; else failures=$((failures + 1)); fi
else
    skip "PyYAML not installed; flatpak manifest not parsed"
fi

# --- staged AppDir ----------------------------------------------------------
if [ -n "$appdir" ]; then
    [ -x "$appdir/AppRun" ] || fail "staged AppDir has no executable AppRun"
    [ -x "$appdir/usr/bin/subordinate" ] || fail "staged AppDir has no subordinate binary"
    [ -f "$appdir/usr/share/applications/$app_id.desktop" ] ||
        fail "staged AppDir has no desktop entry"
    [ -f "$appdir/usr/share/metainfo/$app_id.metainfo.xml" ] ||
        fail "staged AppDir has no metainfo"
    [ -f "$appdir/usr/share/icons/hicolor/scalable/apps/$app_id.svg" ] ||
        fail "staged AppDir has no icon"
    [ -x "$appdir/usr/lib/gstreamer-1.0/gst-plugin-scanner" ] ||
        fail "staged AppDir has no gst-plugin-scanner"
    while read -r plugin; do
        [ -f "$appdir/usr/lib/gstreamer-1.0/libgst$plugin.so" ] ||
            fail "staged AppDir is missing the required plugin $plugin"
    done < <(sed 's/#.*//' "$plugins" | tr -d ' \t' | sed -n 's/^!//p')
    # libc and the driver stack must come from the host: bundling them is what
    # makes an AppImage fail to start, or fall back to software rendering.
    for forbidden in libc.so.6 libstdc++.so.6 libGL.so.1 libEGL.so.1 libva.so.2 \
        libcuda.so.1 libX11.so.6 libwayland-client.so.0; do
        [ -e "$appdir/usr/lib/$forbidden" ] &&
            fail "staged AppDir bundles $forbidden, which must come from the host"
    done
    pass "staged AppDir layout"
fi

echo
if [ "$failures" -eq 0 ]; then
    echo "packaging validation passed ($skipped optional checks skipped)"
    exit 0
fi
echo "packaging validation failed: $failures problem(s)"
exit 1
