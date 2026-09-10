#!/usr/bin/env bash
# Photograph the assembled editor window and its pop-out viewer under Xvfb.
#
# kittest exercises panels in isolation; this exercises the real app: the
# window eframe creates, the wgpu adapter it picks, the dock, and the pop-out
# viewer in a window of its own on a second monitor. Nothing here needs a GPU
# or a seat -- Mesa's lavapipe draws and Xvfb holds the display -- so it runs
# on the free hosted Linux runner (TASK-123).
#
# The two monitors are two Xvfb screens joined with +xinerama, which is what
# makes them one desktop the app can place a window across; the head geometry
# is read back from the server rather than assumed, and each head is cropped
# out of one root capture into its own PNG.
#
# Requires: Xvfb, xdpyinfo (x11-utils), xwd (x11-apps), ImageMagick.
# Output: <out>/screen-0.png, <out>/screen-1.png, <out>/app.log,
#         <out>/summary.md and <out>/screens.txt (one "name WxH" per line).
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
out_dir="$repo_root/target/ui-smoke"
binary=""
project="$repo_root/crates/sub-model/tests/fixtures/sample-project.sub"
display_number=99
# Wide enough that halving one screen still leaves each monitor bigger than
# the editor's minimum window, which is what the fallback below does when the
# server will not give two heads at different origins.
screen_size="2560x800x24"
# The app closes itself after this long, so a capture that never happens
# cannot leave the process running. The wait below is bounded separately.
hold_seconds=25
ready_timeout=120

usage() {
    cat <<'EOF'
Usage: scripts/ui-smoke.sh [options]

  --binary PATH       the subordinate executable (default: cargo run)
  --project PATH      project to open (default: the committed sample project)
  --out DIR           where to write screenshots and the log
  --display N         X display number for Xvfb (default: 99)
  --screen WxHxD      geometry of each of the two screens
  --hold SECONDS      how long the app keeps its windows up
  --timeout SECONDS   how long to wait for the ready line
  -h, --help          show this help
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
    --binary) binary=${2:?--binary needs a path}; shift 2 ;;
    --project) project=${2:?--project needs a path}; shift 2 ;;
    --out) out_dir=${2:?--out needs a directory}; shift 2 ;;
    --display) display_number=${2:?--display needs a number}; shift 2 ;;
    --screen) screen_size=${2:?--screen needs WxHxD}; shift 2 ;;
    --hold) hold_seconds=${2:?--hold needs seconds}; shift 2 ;;
    --timeout) ready_timeout=${2:?--timeout needs seconds}; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "ui-smoke: unknown option $1" >&2; usage >&2; exit 2 ;;
    esac
done

for tool in Xvfb xdpyinfo xwd; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "ui-smoke: $tool is not installed" >&2
        exit 1
    }
done
# ImageMagick 7 renamed convert to magick and keeps convert as a shim.
if command -v magick >/dev/null 2>&1; then
    im() { magick "$@"; }
    im_identify() { magick identify "$@"; }
elif command -v convert >/dev/null 2>&1; then
    im() { convert "$@"; }
    im_identify() { identify "$@"; }
else
    echo "ui-smoke: ImageMagick is not installed" >&2
    exit 1
fi

[ -f "$project" ] || {
    echo "ui-smoke: no project at $project" >&2
    exit 1
}

rm -rf "$out_dir"
mkdir -p "$out_dir"
log="$out_dir/app.log"
display=":$display_number"

xvfb_pid=""
app_pid=""
cleanup() {
    [ -n "$app_pid" ] && kill "$app_pid" 2>/dev/null || true
    [ -n "$xvfb_pid" ] && kill "$xvfb_pid" 2>/dev/null || true
}
trap cleanup EXIT

echo "ui-smoke: starting Xvfb on $display with two $screen_size screens"
Xvfb "$display" -screen 0 "$screen_size" -screen 1 "$screen_size" +xinerama \
    -nolisten tcp >"$out_dir/xvfb.log" 2>&1 &
xvfb_pid=$!

for _ in $(seq 1 50); do
    xdpyinfo -display "$display" >/dev/null 2>&1 && break
    sleep 0.2
done
xdpyinfo -display "$display" >/dev/null 2>&1 || {
    echo "ui-smoke: Xvfb never came up" >&2
    cat "$out_dir/xvfb.log" >&2 || true
    exit 1
}

# Xinerama reports one head per Xvfb screen, at whatever origins the server
# chose. Reading them back rather than assuming a side-by-side layout is what
# keeps the capture honest -- and it is also how the fallback below is
# detected: some builds of Xvfb join the screens at the same origin, which
# would stack both windows in one place and photograph the desktop twice.
heads=$(xdpyinfo -display "$display" -ext XINERAMA |
    sed -n 's/^  head #\([0-9]*\): *\([0-9]*\)x\([0-9]*\) @ \([0-9-]*\),\([0-9-]*\).*/\2 \3 \4 \5/p')
head_count=$(printf '%s\n' "$heads" | grep -c . || true)
first_origin=$(printf '%s\n' "$heads" | sed -n '1p' | awk '{print $3, $4}')
second_origin=$(printf '%s\n' "$heads" | sed -n '2p' | awk '{print $3, $4}')
if [ "$head_count" -lt 2 ] || [ "$first_origin" = "$second_origin" ]; then
    # No usable two-head desktop from Xinerama. Split the first screen into
    # two RandR monitors instead: the desktop is the same size either way, and
    # a monitor with no output is exactly how a virtual second display is
    # emulated on a headless server.
    echo "ui-smoke: Xinerama gave $head_count usable head(s); splitting screen 0 in two"
    root_size=$(xdpyinfo -display "$display" |
        sed -n 's/^  dimensions: *\([0-9]*\)x\([0-9]*\).*/\1 \2/p' | head -1)
    root_width=${root_size% *}
    root_height=${root_size#* }
    half=$((root_width / 2))
    if command -v xrandr >/dev/null 2>&1; then
        xrandr -display "$display" --setmonitor SUB-0 "$half/0x$root_height/0+0+0" none || true
        xrandr -display "$display" --setmonitor SUB-1 "$half/0x$root_height/0+$half+0" none || true
    fi
    heads=$(printf '%s %s 0 0\n%s %s %s 0\n' "$half" "$root_height" "$half" "$root_height" "$half")
fi
echo "ui-smoke: heads (WxH @ x,y)"
printf '%s\n' "$heads" | awk '{printf "  head %d: %sx%s @ %s,%s\n", NR - 1, $1, $2, $3, $4}'
second_x=$(printf '%s\n' "$heads" | sed -n '2p' | awk '{print $3}')
second_y=$(printf '%s\n' "$heads" | sed -n '2p' | awk '{print $4}')

# Software Vulkan, no GPU: the picture is what a headless runner can draw, and
# the point is that the windows come up at all.
export LIBGL_ALWAYS_SOFTWARE=1
export RUST_LOG="${RUST_LOG:-info}"
export DISPLAY="$display"

if [ -n "$binary" ]; then
    set -- "$binary"
else
    set -- cargo run --quiet -p subordinate --
fi
echo "ui-smoke: launching $* with the pop-out at $second_x,$second_y"
"$@" --ui-smoke --hold-seconds "$hold_seconds" \
    --popout-position "$second_x,$second_y" "$project" >"$log" 2>&1 &
app_pid=$!

# The app prints its ready line once the editor window and the pop-out have
# each painted a frame; capturing before that photographs empty rectangles.
ready=0
for _ in $(seq 1 $((ready_timeout * 5))); do
    if grep -q 'ui-smoke ready' "$log" 2>/dev/null; then
        ready=1
        break
    fi
    kill -0 "$app_pid" 2>/dev/null || break
    sleep 0.2
done
if [ "$ready" -ne 1 ]; then
    echo "ui-smoke: the app never reported a first frame" >&2
    cat "$log" >&2 || true
    exit 1
fi
ready_line=$(grep -m1 'ui-smoke ready' "$log")
echo "ui-smoke: $ready_line"
case "$ready_line" in
*project=loaded*) ;;
*)
    echo "ui-smoke: the sample project did not open" >&2
    exit 1
    ;;
esac

# One capture of the whole Xinerama desktop, then a crop per head: two xwd
# runs could catch the two windows a frame apart.
xwd -display "$display" -root -silent >"$out_dir/root.xwd"
index=0
: >"$out_dir/screens.txt"
while read -r width height x y; do
    [ -n "$width" ] || continue
    png="$out_dir/screen-$index.png"
    im "xwd:$out_dir/root.xwd" -crop "${width}x${height}+${x}+${y}" +repage "$png"
    echo "screen-$index.png $(im_identify -format '%wx%h' "$png")" >>"$out_dir/screens.txt"
    index=$((index + 1))
done <<EOF
$heads
EOF
rm -f "$out_dir/root.xwd"

# The pictures are taken, so the run is over: closing the app here rather than
# sitting out the rest of its hold is most of what keeps this step short. The
# hold is the insurance against a capture that never happens, not the schedule.
kill "$app_pid" 2>/dev/null || true
wait "$app_pid" 2>/dev/null || true
app_pid=""

{
    echo "### Window smoke test"
    echo
    echo "| file | dimensions |"
    echo "| --- | --- |"
    while read -r name size; do
        echo "| \`$name\` | $size |"
    done <"$out_dir/screens.txt"
    echo "| \`app.log\` | $(wc -l <"$log" | tr -d ' ') lines |"
    echo
    echo "Head 0 is the editor window, head 1 the pop-out viewer."
    echo
    echo '```'
    echo "$ready_line"
    echo '```'
} >"$out_dir/summary.md"

cat "$out_dir/screens.txt"
echo "ui-smoke: wrote $out_dir"
