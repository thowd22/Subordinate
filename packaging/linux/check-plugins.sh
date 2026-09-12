#!/bin/sh
# Load every plugin the AppImage bundles, through the AppImage, and fail on the
# first one that will not load.
#
# This is the check the build itself cannot make. build-appimage.sh runs `ldd`
# over the staged plugins, but it runs it on the machine that built them, where
# the GStreamer development packages have already dragged in every library the
# bundle links against -- so a dependency the *package* does not carry resolves
# anyway and the plugin looks fine. On a user's machine the scanner then fails
# to load it, blacklists it, prints a line nobody reads and carries on with the
# element simply missing: that is how the v0.1.2 AppImage shipped with `libav`,
# `va`, `qsv` and `msdk` all dead for want of libva-drm.so.2, and an ordinary
# H.264/AAC file answering media.unsupported (TASK-139, TASK-145).
#
# So run it somewhere that did not build anything: a stock ubuntu:24.04 or
# fedora:41 container carrying only the host half of the split -- the graphics,
# display and audio client libraries build-appimage.sh deliberately excludes.
# Anything missing there is a bundling bug.
#
# Usage:
#   packaging/linux/check-plugins.sh <AppImage|AppDir> [options]
#
#   --skip LIST   comma-separated plugin names that may fail here (none by
#                 default; a plugin that needs a device is still expected to
#                 *load*, it just has no elements to offer)
#   -h, --help    this text
#
# POSIX sh: it runs inside the same bare containers the package has to run in,
# which have no bash.
set -eu

target=""
skip=""
while [ $# -gt 0 ]; do
    case "$1" in
    --skip) skip=${2:?--skip needs a list} && shift 2 ;;
    -h | --help)
        sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    -*)
        echo "check-plugins.sh: unknown option $1" >&2
        exit 2
        ;;
    *) target=$1 && shift ;;
    esac
done
[ -n "$target" ] || {
    echo "check-plugins.sh: name the AppImage or the staged AppDir" >&2
    exit 2
}

# Either form is accepted so the same script can check a staged AppDir during
# a build and the packed artifact afterwards. An AppImage needs FUSE to mount
# itself and a container has no /dev/fuse, so it is unpacked instead -- the
# identical AppDir, and one unsquash rather than one per plugin.
work=""
if [ -d "$target" ]; then
    appdir=$(cd "$target" && pwd)
else
    [ -f "$target" ] || {
        echo "check-plugins.sh: no such file: $target" >&2
        exit 2
    }
    chmod +x "$target" 2>/dev/null || true
    work=${TMPDIR:-/tmp}/check-plugins-$$
    mkdir -p "$work"
    target=$(cd "$(dirname "$target")" && pwd)/$(basename "$target")
    (cd "$work" && "$target" --appimage-extract >/dev/null)
    appdir=$work/squashfs-root
fi
[ -x "$appdir/AppRun" ] || {
    echo "check-plugins.sh: $appdir has no AppRun" >&2
    exit 2
}
cleanup() { [ -n "$work" ] && rm -rf "$work"; }
trap cleanup EXIT INT TERM

plugin_dir=$appdir/usr/lib/gstreamer-1.0
[ -d "$plugin_dir" ] || {
    echo "check-plugins.sh: $plugin_dir is not a directory" >&2
    exit 2
}

# Everything runs through AppRun rather than against the .so directly: AppRun
# is what sets GST_PLUGIN_SCANNER, the private registry and the library path,
# and on a host missing one of the VA-API dispatchers it is also what links the
# bundled fallback in. Checking the plugins any other way would check a
# configuration no user ever runs.
inspect() {
    SUB_APPIMAGE_TOOL=gst-inspect-1.0 "$appdir/AppRun" "$@"
}

if [ -r /etc/os-release ]; then
    # shellcheck source=/dev/null
    (. /etc/os-release && echo "machine: ${PRETTY_NAME:-$NAME}")
fi
echo "glibc:   $(ldd --version 2>&1 | head -n1)"
echo "package: $appdir"
inspect --version | head -n2

failed=""
skipped=""
checked=0
for so in "$plugin_dir"/libgst*.so; do
    [ -f "$so" ] || continue
    name=$(basename "$so" .so)
    name=${name#libgst}
    checked=$((checked + 1))
    if inspect "$name" >/dev/null 2>&1; then
        echo "ok         $name"
        continue
    fi
    case ",$skip," in
    *",$name,"*)
        echo "skipped    $name (allowed to fail here)"
        skipped="$skipped $name"
        continue
        ;;
    esac
    echo "WILL NOT LOAD  $name"
    # The scanner says exactly which library it could not find, which is the
    # whole answer to "what is missing from this package". Printed for every
    # failure, because the first one is rarely the only one.
    GST_DEBUG=GST_PLUGIN_LOADING:4 inspect "$name" 2>&1 |
        grep -iE 'cannot open shared object|undefined symbol|blacklist|failed to load|not found' |
        sed 's/^/    /' | head -n 10 || true
    failed="$failed $name"
done

echo "checked $checked bundled plugins"

# A plugin the scanner rejected is blacklisted rather than absent, and the
# registry says so in one line at the end of a bare gst-inspect. Belt and
# braces: a module can be blacklisted for a reason that still lets
# `gst-inspect <name>` print something.
summary=$(inspect 2>/dev/null | tail -n1)
echo "registry: $summary"

status=0
if [ -n "$failed" ]; then
    echo "::error::the package cannot load its own plugins:$failed"
    status=1
fi
case "$summary" in
*blacklist*)
    echo "::error::the bundled registry has blacklist entries: $summary"
    status=1
    ;;
esac
if [ -n "$skipped" ]; then
    echo "note: allowed to fail:$skipped"
fi
if [ "$status" -eq 0 ]; then
    echo "every bundled plugin loads on this machine"
fi
exit "$status"
