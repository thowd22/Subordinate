#!/bin/sh
# Verify an *installed* Subordinate package on a machine that has never built
# the project (TASK-110).
#
# Nothing here compiles, and nothing here reaches into a source tree: every
# command runs out of the installed package, and the only inputs are the
# sample project and its media. That is the whole point -- the packaging
# workflows smoke-test their packages on the machine that just built them,
# which cannot tell a package that carries its dependencies from one that
# found them lying around.
#
# The package is reached through an *adapter*: a tiny shell file that defines
# three functions, so the sequence below is written once and an AppImage, a
# Flatpak, a deb or a tarball can each be checked by it.
#
#   sub_cli  <args...>          run subordinate-cli from the package
#   sub_tool <tool> <args...>   run a bundled GStreamer tool (gst-inspect-1.0,
#                               gst-discoverer-1.0)
#   sub_gui  <args...>          run the editor itself
#
# .github/workflows/fresh-install.yml writes one adapter per package; the
# runbook in docs/DEVELOPMENT.md ("Fresh-machine install verification") shows
# the two-line adapter for checking a download by hand.
#
# Output: a line-by-line log on stdout and <out>/facts.txt, a key=value record
# of the machine, the package and the result.
set -eu

adapter=""
project=""
out_dir=""
preset="mezzanine"
sequence="Main cut"
range="0:25"
want_gui=1
strict=0

usage() {
    cat <<'EOF'
Usage: fresh-install-check.sh --adapter FILE --project FILE [options]

  --adapter FILE   shell file defining sub_cli, sub_tool and sub_gui
  --project FILE   the sample project to open (its media beside it)
  --out DIR        where to write facts.txt and the render (default: ./fresh-install)
  --preset ID      export preset (default: mezzanine -- H.264 in MKV with FLAC
                   audio, so no machine is failed for want of an AAC encoder)
  --sequence NAME  which sequence to render (default: Main cut)
  --range IN:OUT   frames of the sequence timebase (default: 0:25)
  --no-gui         skip opening the editor window
  --strict         fail if this machine has a Rust toolchain or a GStreamer of
                   its own; for a genuinely clean machine, not for a developer
                   box or a self-hosted runner
  -h, --help       show this help
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
    --adapter) adapter=${2:?--adapter needs a file} && shift 2 ;;
    --project) project=${2:?--project needs a file} && shift 2 ;;
    --out) out_dir=${2:?--out needs a directory} && shift 2 ;;
    --preset) preset=${2:?--preset needs an id} && shift 2 ;;
    --sequence) sequence=${2:?--sequence needs a name} && shift 2 ;;
    --range) range=${2:?--range needs IN:OUT} && shift 2 ;;
    --no-gui) want_gui=0 && shift ;;
    --strict) strict=1 && shift ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        echo "fresh-install-check: unknown option '$1'" >&2
        usage >&2
        exit 2
        ;;
    esac
done

[ -n "$adapter" ] || {
    echo "fresh-install-check: --adapter is required" >&2
    exit 2
}
[ -n "$project" ] || {
    echo "fresh-install-check: --project is required" >&2
    exit 2
}
[ -f "$project" ] || {
    echo "fresh-install-check: no such project: $project" >&2
    exit 2
}
[ -n "$out_dir" ] || out_dir=$PWD/fresh-install
mkdir -p "$out_dir"

facts=$out_dir/facts.txt
: >"$facts"
fact() { echo "$1=$2" | tee -a "$facts"; }

banner() {
    echo
    echo "=============================================================="
    echo "== $1"
    echo "=============================================================="
}

fail() {
    echo "::error::$1"
    fact result fail
    fact failed_at "$2"
    exit 1
}

# shellcheck source=/dev/null
. "$adapter"
for required in sub_cli sub_tool sub_gui; do
    command -v "$required" >/dev/null 2>&1 ||
        fail "the adapter $adapter does not define $required" adapter
done
label=${SUB_PACKAGE_LABEL:-package}
fact package "$label"

# ------------------------------------------------------------- the machine --
banner "the machine"
os="unknown"
if [ -r /etc/os-release ]; then
    # shellcheck source=/dev/null
    . /etc/os-release
    os=${PRETTY_NAME:-${NAME:-unknown}}
elif [ "$(uname -s)" = "Darwin" ]; then
    os="macOS $(sw_vers -productVersion 2>/dev/null || echo unknown)"
fi
fact os "$os"
fact kernel "$(uname -sr)"
fact libc "$(ldd --version 2>&1 | head -n1)"
fact arch "$(uname -m)"
if [ -d /dev/dri ]; then
    fact gpu "render nodes: $(ls /dev/dri 2>/dev/null | tr '\n' ' ')"
else
    fact gpu "none (no /dev/dri on this machine)"
fi

# A machine that can build the project proves nothing about a package that has
# to carry its own dependencies, so on a machine that claims to be clean these
# are failures rather than notes.
for unwanted in cargo rustc gst-inspect-1.0 gst-launch-1.0; do
    if command -v "$unwanted" >/dev/null 2>&1; then
        if [ "$strict" -eq 1 ]; then
            fail "this machine already has $unwanted; it is not a fresh machine" machine
        fi
        echo "note: this machine has its own $unwanted ($(command -v "$unwanted")); the package is still used exclusively"
    else
        echo "absent, as a fresh machine should be: $unwanted"
    fi
done
if [ "$strict" -eq 1 ] && command -v pkg-config >/dev/null 2>&1; then
    if pkg-config --exists gstreamer-1.0 2>/dev/null; then
        fail "this machine has GStreamer development files" machine
    fi
fi

# ------------------------------------------------------- the package starts --
banner "the installed package starts"
sub_gui --help >"$out_dir/help.txt" || fail "the editor would not start" start
head -n3 "$out_dir/help.txt"
sub_tool gst-inspect-1.0 --version >"$out_dir/gst-version.txt" ||
    fail "the bundled gst-inspect-1.0 would not run" start
cat "$out_dir/gst-version.txt"
fact gstreamer "$(sed -n 's/^GStreamer //p' "$out_dir/gst-version.txt" | head -n1)"

# gst-discoverer-1.0 is a separate binary from the library the editor probes
# with, and not every package ships it: on Ubuntu it lives in
# gstreamer1.0-plugins-base-apps, so an AppImage built without that package has
# the discoverer *library* and no discoverer *command* (found by this check,
# run 34641628065). Where the command is there it reads the export back from
# outside, which is an independent look at the file; where it is not, the
# render's own --verify probe -- the same discoverer, from the same bundle,
# in process -- is what the result rests on. Either way the check says which.
if sub_tool gst-discoverer-1.0 --help >/dev/null 2>&1; then
    has_discoverer=1
    fact discoverer "bundled as a command"
else
    has_discoverer=0
    fact discoverer "library only; --verify probes in process"
fi

# A plugin the scanner blacklisted is a library whose dependencies did not come
# along -- exactly what only shows up on a machine that did not build it.
sub_tool gst-inspect-1.0 >"$out_dir/gst-plugins.txt" 2>/dev/null || true
summary=$(tail -n1 "$out_dir/gst-plugins.txt")
echo "$summary"
case "$summary" in
*blacklist*) fail "the installed runtime blacklisted some of its own plugins: $summary" plugins ;;
esac

# ------------------------------------------------------- open the project ----
banner "open the sample project"
sub_cli open "$project" >"$out_dir/open.json" || fail "the project would not open" open
cat "$out_dir/open.json"
# `offline` lists every media file the project refers to that could not be
# found or whose bytes are not the ones it was authored against. A relink
# prompt on a fresh machine is the classic packaging failure: a path that only
# resolved because of where the build tree happened to be.
grep -q '"offline": \[\]' "$out_dir/open.json" ||
    fail "the project opened with offline media; see open.json" open
echo "every media path resolved: no relink needed"

sub_cli inspect "$project" >"$out_dir/inspect.json" || fail "the project would not inspect" open
echo "inspect.json: $(wc -c <"$out_dir/inspect.json") bytes"

# ------------------------------------------------------------ probe media ----
banner "probe the media"
media_dir=$(dirname "$project")/media
probed=0
for file in "$media_dir"/*.webm; do
    [ -f "$file" ] || continue
    name=$(basename "$file")
    probed=$((probed + 1))
    [ "$has_discoverer" -eq 1 ] || continue
    sub_tool gst-discoverer-1.0 "$file" >"$out_dir/probe-$name.txt" 2>&1 ||
        fail "the bundled discoverer could not read $name" probe
    if grep -qi 'error' "$out_dir/probe-$name.txt"; then
        fail "the bundled discoverer reported an error on $name" probe
    fi
    echo "--- $name"
    sed -n '/Topology/,/^$/p' "$out_dir/probe-$name.txt" | head -n12
done
[ "$probed" -gt 0 ] || fail "no sample media was found beside $project" probe
fact media_probed "$probed"
if [ "$has_discoverer" -eq 1 ]; then
    # One of the three clips carries the music bed; an audio stream the package
    # can decode is what the mix below is made of.
    grep -qi 'audio' "$out_dir"/probe-*.txt || fail "no audio stream in any sample clip" probe
    echo "$probed clips read by the bundled discoverer, audio streams included"
else
    echo "$probed clips present; the export below decodes every one of them"
fi

# --------------------------------------------------------------- export -----
banner "export with the best available encoder"
# sub_export's own selection order (crates/sub-export/src/encoder.rs): hardware
# first, software last. On a hosted runner only x264enc survives, which is the
# honest answer for a machine with no GPU; on a machine with one the hardware
# encoder is used instead, and the summary says which.
available=""
for element in nvh264enc vah264enc amfh264enc mfh264enc x264enc; do
    if sub_tool gst-inspect-1.0 --exists "$element" >/dev/null 2>&1; then
        available="$available $element"
        echo "encoder present: $element"
    else
        echo "encoder absent:  $element"
    fi
done
fact encoders_available "$(echo "$available" | sed 's/^ *//')"
[ -n "$available" ] || fail "the installed runtime offers no H.264 encoder at all" export

case "$preset" in
mezzanine | h265-archive) extension=mkv ;;
*) extension=mp4 ;;
esac
output=$out_dir/fresh-install.$extension
used=""
for element in $available; do
    echo "--- rendering with $element"
    if sub_cli render "$project" --sequence "$sequence" --preset "$preset" \
        --encoder "$element" --range "$range" --out "$output" --verify \
        >"$out_dir/render.json" 2>"$out_dir/render.log"; then
        used=$element
        break
    fi
    # Present in the registry is not the same as usable: nvcodec and va both
    # register elements that need a driver behind them, so falling through to
    # the next candidate is what "best *available*" means.
    echo "$element could not render on this machine:"
    tail -n20 "$out_dir/render.log"
done
[ -n "$used" ] || fail "no available encoder could render the sample project" export
fact encoder_used "$used"
cat "$out_dir/render.json"

# --------------------------------------------------------------- validate ---
banner "validate the export"
[ -f "$output" ] || fail "the render reported success but wrote no file" validate
bytes=$(wc -c <"$output" | tr -d ' ')
fact output_bytes "$bytes"
echo "$output is $bytes bytes"
[ "$bytes" -gt 10240 ] || fail "the rendered file is implausibly small ($bytes bytes)" validate

# --verify made the package read its own output back before it exited, and
# refuses a file with no video; that report is in render.json either way.
grep -q '"probe"' "$out_dir/render.json" ||
    fail "the render did not probe what it wrote (--verify produced no report)" validate
sed -n '/"audio"/,/]/p' "$out_dir/render.json" | grep -q '"codec"' ||
    fail "the render's own probe found no audio stream in the export" validate
echo "the package read its own export back: video and audio both present"

if [ "$has_discoverer" -eq 1 ]; then
    sub_tool gst-discoverer-1.0 "$output" >"$out_dir/discoverer.txt" 2>&1 ||
        fail "the bundled discoverer could not read the file the package just wrote" validate
    cat "$out_dir/discoverer.txt"
    if grep -qi 'error' "$out_dir/discoverer.txt"; then
        fail "the discoverer reported an error on the export" validate
    fi
    grep -qi 'video' "$out_dir/discoverer.txt" || fail "the export carries no video stream" validate
    grep -qi 'audio' "$out_dir/discoverer.txt" || fail "the export carries no audio stream" validate
    echo "and the bundled gst-discoverer-1.0 reads it back from outside: video and audio"
fi

# ------------------------------------------------------------------ the UI --
if [ "$want_gui" -eq 1 ]; then
    banner "open the project in the editor"
    xvfb_pid=""
    if [ -z "${DISPLAY:-}" ] && command -v Xvfb >/dev/null 2>&1; then
        # A container has no display. Xvfb is the machine's X server for the
        # length of this check, exactly as scripts/ui-smoke.sh uses it; the
        # screen is wide enough that the pop-out viewer has somewhere to go.
        Xvfb :99 -screen 0 2560x800x24 -nolisten tcp >"$out_dir/xvfb.log" 2>&1 &
        xvfb_pid=$!
        DISPLAY=:99
        export DISPLAY
        waited=0
        while [ ! -e /tmp/.X11-unix/X99 ] && [ "$waited" -lt 60 ]; do
            sleep 0.5
            waited=$((waited + 1))
        done
        [ -e /tmp/.X11-unix/X99 ] || fail "Xvfb never came up" gui
    fi
    if [ -z "${DISPLAY:-}" ] && [ "$(uname -s)" != "Darwin" ]; then
        echo "no display and no Xvfb: skipping the window check"
        fact gui "skipped (no display)"
    else
        # No GPU here, so the software rasteriser is asked for by name: Mesa's
        # llvmpipe for GL and lavapipe for Vulkan, which is what wgpu picks up.
        LIBGL_ALWAYS_SOFTWARE=1
        export LIBGL_ALWAYS_SOFTWARE
        lavapipe=$(ls /usr/share/vulkan/icd.d/lvp_icd.*.json 2>/dev/null | head -n1 || true)
        if [ -n "$lavapipe" ]; then
            VK_DRIVER_FILES=$lavapipe
            VK_ICD_FILENAMES=$lavapipe # the name the loader used before 1.3.207
            export VK_DRIVER_FILES VK_ICD_FILENAMES
            echo "software Vulkan: $lavapipe"
        fi
        status=0
        sub_gui --ui-smoke "$project" --hold-seconds 5 >"$out_dir/ui.log" 2>&1 || status=$?
        if [ -n "$xvfb_pid" ]; then kill "$xvfb_pid" 2>/dev/null || true; fi
        tail -n40 "$out_dir/ui.log"
        [ "$status" -eq 0 ] || fail "the editor exited $status opening the project" gui
        grep -q 'ui-smoke ready' "$out_dir/ui.log" ||
            fail "the editor never reported its windows ready" gui
        fact gui ok
        echo "the editor opened the project, painted and popped the viewer out"
    fi
else
    fact gui "not requested"
fi

fact result pass
banner "PASS -- $label installed and ran on $os"
