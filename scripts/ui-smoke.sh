#!/usr/bin/env bash
# Photograph the assembled editor window and its pop-out viewer under Xvfb.
#
# kittest exercises panels in isolation; this exercises the real app: the
# window eframe creates, the wgpu adapter it picks, the dock, and the pop-out
# viewer in a window of its own on a second monitor. Nothing here needs a GPU
# or a seat -- Mesa's lavapipe draws and Xvfb holds the display -- so it runs
# on the free hosted Linux runner (TASK-123).
#
# The two heads are carved out of one wide Xvfb screen, which is what makes
# them one desktop the app can place a window across; a second X screen joined
# with Xinerama looks the same but breaks window coordinate translation under
# winit. Two RandR monitors are asked for first, since that is what makes the
# heads visible to the app's own display enumeration, and the server's answer
# is recorded either way (Ubuntu 26.04's Xvfb takes the request and creates
# nothing). Each head is cropped out of one root capture into its own PNG, with
# whichever window landed on it outlined and named.
#
# --gpu asks for the machine's real adapter; a display that cannot present it
# (Xvfb has no DRI3, so Mesa refuses the surface) falls back to lavapipe with
# the reason printed. --require-popout-on-head N turns the pop-out's placement
# from a picture someone has to look at into an assertion, by reading the
# window's absolute geometry back off the server and checking it lies inside
# that head's rectangle (TASK-118).
#
# Requires: Xvfb, xdpyinfo and xwininfo (x11-utils), xwd (x11-apps), ImageMagick.
# Output: <out>/screen-0.png, <out>/screen-1.png, <out>/app.log,
#         <out>/summary.md, <out>/screens.txt (one "name WxH" per line),
#         <out>/windows.txt (the top-level windows with their geometry) and
#         <out>/monitors.txt (the server's RandR version, monitors and heads).
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
out_dir="$repo_root/target/ui-smoke"
binary=""
# The demo project when its media has been fetched, and the committed fixture
# otherwise. The fixture names footage nothing generates, so a window opened on
# it has nothing to decode and photographs a black viewer; the demo project's
# three CC0 clips are what make the screenshot show a picture (TASK-144).
# `scripts/get-sample-media.sh` puts them there, which CI runs before this.
project=""
demo_project="$repo_root/examples/sample-project/demo.sub"
fixture_project="$repo_root/crates/sub-model/tests/fixtures/sample-project.sub"
display_number=99
# Wide enough that halving one screen still leaves each monitor bigger than
# the editor's minimum window, which is what the fallback below does when the
# server will not give two heads at different origins.
screen_size="2560x800x24"
# The app closes itself after this long, so a capture that never happens
# cannot leave the process running. The wait below is bounded separately.
hold_seconds=25
ready_timeout=120
# Hosted CI has no GPU, so the default stays on the software rasteriser; a job
# on a machine with a real one passes --gpu.
force_software=1
# Empty means "photograph it and say where it landed"; a head index means
# "fail the run if the pop-out is not on that head".
require_popout_head=""
# Whether a black viewer is a failure. Off by default, because a run pointed at
# a project whose media is not on this machine has nothing to show; CI turns it
# on for the run that fetched the media.
require_picture=0


usage() {
    cat <<'EOF'
Usage: scripts/ui-smoke.sh [options]

  --binary PATH       the subordinate executable (default: cargo run)
  --project PATH      project to open (default: the demo project when its
                      media has been fetched, else the committed fixture)
  --require-picture   fail unless the viewer composited a decoded picture
  --out DIR           where to write screenshots and the log
  --display N         X display number for Xvfb (default: 99)
  --screen WxHxD      geometry of each of the two screens
  --hold SECONDS      how long the app keeps its windows up
  --timeout SECONDS   how long to wait for the ready line
  --gpu               do not force the software rasteriser (real GPU present)
  --require-popout-on-head N
                      fail unless the pop-out window landed on head N
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
    --gpu) force_software=0; shift ;;
    --require-picture) require_picture=1; shift ;;
    --require-popout-on-head)
        require_popout_head=${2:?--require-popout-on-head needs a head index}; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "ui-smoke: unknown option $1" >&2; usage >&2; exit 2 ;;
    esac
done

for tool in Xvfb xdpyinfo xwd xwininfo; do
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

if [ -z "$project" ]; then
    if [ -f "$demo_project" ] && [ -d "$repo_root/examples/sample-project/media" ]; then
        project=$demo_project
        echo "ui-smoke: opening the demo project; its media is on this machine"
    else
        project=$fixture_project
        echo "ui-smoke: the demo project's media is not here (run scripts/get-sample-media.sh); opening the committed fixture, whose viewer has nothing to decode"
    fi
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

# One screen, no +xinerama. Two Xvfb screens joined by Xinerama put the app's
# window on a PanoramiX screen whose root XTranslateCoordinates refuses, which
# winit unwraps -- the app panicked on its first paint before it could report a
# frame. A single wide screen split into two RandR monitors below gives the
# same two-head desktop on the server layout the plain GUI smoke test already
# proves works.
echo "ui-smoke: starting Xvfb on $display with one $screen_size screen"
Xvfb "$display" -screen 0 "$screen_size" -nolisten tcp \
    >"$out_dir/xvfb.log" 2>&1 &
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
        # A RandR monitor with no output is how the protocol expresses a head
        # that is not physically there, which is exactly a virtual second
        # display. Which spelling of `--setmonitor` a given server accepts is
        # not something to guess at, though: on box's Xvfb the first attempt
        # below left the server with its automatic whole-screen monitor and
        # said nothing about why (run 34615366090). So the spellings are tried
        # in turn and each one is checked against the server's own monitor
        # list, with everything xrandr said kept in the log.
        #
        # `--setmonitor` wants physical dimensions, and xrandr reads 0mm as
        # "not specified", so they come from the pixel count at 96 dpi.
        output=$(xrandr -display "$display" --listmonitors |
            awk 'NR > 1 { print $NF; exit }')
        [ -n "$output" ] || output=none
        mm_w=$((half * 254 / 960))
        mm_h=$((root_height * 254 / 960))
        geom_0="$half/${mm_w}x$root_height/$mm_h+0+0"
        geom_1="$half/${mm_w}x$root_height/$mm_h+$half+0"

        monitor_count() {
            xrandr -display "$display" --listmonitors 2>/dev/null |
                sed -n 's/^Monitors: \([0-9]*\).*/\1/p' | head -1
        }
        try_split() {
            echo "ui-smoke: monitor split attempt: $*"
            set +e
            "$@" >"$out_dir/setmonitor.log" 2>&1
            set -e
            sed 's/^/ui-smoke: xrandr: /' "$out_dir/setmonitor.log" || true
            [ "$(monitor_count)" -ge 2 ] 2>/dev/null
        }
        xrandr -display "$display" --version 2>&1 | sed 's/^/ui-smoke: /' || true
        # 1. one process, both monitors, the real output on the left head.
        # 2. the same in two processes.
        # 3. only the right-hand head, leaving the server's automatic monitor
        #    as the left one - fewer requests, and the automatic monitor keeps
        #    whatever the server thinks its output really is.
        try_split xrandr -display "$display" \
            --setmonitor SUB-0 "$geom_0" "$output" \
            --setmonitor SUB-1 "$geom_1" none ||
            try_split xrandr -display "$display" --setmonitor SUB-1 "$geom_1" none ||
            try_split xrandr -display "$display" --setmonitor SUB-0 "$geom_0" none ||
            echo "ui-smoke: no spelling of --setmonitor gave this server two monitors"
    fi
    heads=$(printf '%s %s 0 0\n%s %s %s 0\n' "$half" "$root_height" "$half" "$root_height" "$half")
fi
# What the server itself says its outputs are, kept as evidence beside the
# screenshots: a job that claims a two-output desktop should be able to show
# the monitor list that proves it (TASK-118).
{
    if command -v xrandr >/dev/null 2>&1; then
        xrandr -display "$display" --version 2>&1
        xrandr -display "$display" --listmonitors 2>&1
    else
        echo "xrandr is not installed; no monitor list"
    fi
    echo "--- XINERAMA ---"
    # `| head` would close the pipe under the caller's `set -o pipefail` and
    # take the whole script with it, so the trimming happens inside sed.
    xdpyinfo -display "$display" -ext XINERAMA 2>&1 |
        sed -n '/head #/p; /dimensions:/p; /number of screens/p' || true
    echo "--- heads this run uses ---"
    printf '%s\n' "$heads" | awk '{printf "head %d: %sx%s @ %s,%s\n", NR - 1, $1, $2, $3, $4}'
} >"$out_dir/monitors.txt" 2>&1
cat "$out_dir/monitors.txt"

echo "ui-smoke: heads (WxH @ x,y)"
printf '%s\n' "$heads" | awk '{printf "  head %d: %sx%s @ %s,%s\n", NR - 1, $1, $2, $3, $4}'
second_x=$(printf '%s\n' "$heads" | sed -n '2p' | awk '{print $3}')
second_y=$(printf '%s\n' "$heads" | sed -n '2p' | awk '{print $4}')

# A bare X root is black, and so is a viewer with nothing to show, so a
# screenshot of the pop-out on an empty desktop is a black rectangle on a black
# field and proves nothing to the eye. Painting the root a colour no part of
# the editor uses makes each window's rectangle obvious in its own screenshot.
if command -v xsetroot >/dev/null 2>&1; then
    xsetroot -display "$display" -solid '#1d4f7c' || true
fi

export RUST_LOG="${RUST_LOG:-info}"
export DISPLAY="$display"

if [ -n "$binary" ]; then
    set -- "$binary"
else
    set -- cargo run --quiet -p subordinate --
fi

# launch_app -> 0 once the app has reported its first frame, 1 if it died or
# never got there. The log is truncated per attempt, because there is at most
# one retry and the failed attempt is kept beside it.
launch_app() {
    "$@" --ui-smoke --hold-seconds "$hold_seconds" \
        --popout-position "$second_x,$second_y" "$project" >"$log" 2>&1 &
    app_pid=$!
    for _ in $(seq 1 $((ready_timeout * 5))); do
        if grep -q 'ui-smoke ready' "$log" 2>/dev/null; then
            return 0
        fi
        kill -0 "$app_pid" 2>/dev/null || break
        sleep 0.2
    done
    return 1
}

# Vulkan on a real GPU cannot present into an Xvfb window: Mesa's WSI needs
# DRI3, which Xvfb does not implement, so RADV refuses the surface with "There
# was no valid format for the surface at all" before the first frame (run
# 34614668762 on box). The GPU's own rendering is covered by the headless
# readback and render jobs in this workflow; what this script is for is the
# windows. So --gpu means "draw with the real adapter if this display can
# present it", and a display that cannot falls back to the software rasteriser
# with the reason said out loud rather than failing the run.
#
# LIBGL_ALWAYS_SOFTWARE is a GL variable and says nothing to Vulkan, which is
# what the compositor actually uses: on a machine with more than one ICD the
# loader still hands out the discrete driver (run 34615366090 picked RADV
# twice). The Vulkan half is the loader's own driver-file override, pointed at
# Mesa's lavapipe.
use_software_rasteriser() {
    export LIBGL_ALWAYS_SOFTWARE=1
    lvp=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1 || true)
    if [ -n "$lvp" ]; then
        export VK_DRIVER_FILES="$lvp"
        export VK_ICD_FILENAMES="$lvp" # the name the loader used before 1.3.207
        echo "ui-smoke: software rasteriser: $lvp"
    else
        echo "ui-smoke: no lavapipe ICD installed; the loader will pick what it likes"
    fi
}

software=$force_software
if [ "$software" -eq 1 ]; then
    use_software_rasteriser
else
    unset LIBGL_ALWAYS_SOFTWARE VK_DRIVER_FILES VK_ICD_FILENAMES || true
fi
echo "ui-smoke: launching $* with the pop-out at $second_x,$second_y"
if ! launch_app "$@"; then
    if [ "$software" -eq 0 ] && grep -qi 'no valid format for the surface\|Found no drivers\|no suitable adapter' "$log"; then
        echo "ui-smoke: the real adapter could not present on $display (an X server without DRI3, such as Xvfb); retrying on the software rasteriser"
        cp "$log" "$out_dir/app-gpu-attempt.log"
        kill "$app_pid" 2>/dev/null || true
        wait "$app_pid" 2>/dev/null || true
        app_pid=""
        software=1
        use_software_rasteriser
        launch_app "$@" || {
            echo "ui-smoke: the app never reported a first frame" >&2
            cat "$log" >&2 || true
            exit 1
        }
    else
        echo "ui-smoke: the app never reported a first frame" >&2
        cat "$log" >&2 || true
        exit 1
    fi
fi
ready_line=$(grep -m1 'ui-smoke ready' "$log")
echo "ui-smoke: $ready_line"
# Which wgpu adapter drew the picture. On a GPU runner this is the evidence
# that the run was not quietly served by a software rasteriser.
adapter_line=$(grep -m1 'adapter chosen' "$log" || true)
[ -n "$adapter_line" ] && echo "ui-smoke: $adapter_line" || true
case "$ready_line" in
*project=loaded*) ;;
*)
    echo "ui-smoke: the sample project did not open" >&2
    exit 1
    ;;
esac
# The window edits through the engine that owns the project, so the counts on
# the ready line are read back out of that engine's snapshot. A window that
# came up on an empty project would still say project=loaded; these say the
# sequence the panels are showing really came from the file.
case "$ready_line" in
*sequences=0* | *tracks=0*)
    echo "ui-smoke: the window came up on an empty project" >&2
    exit 1
    ;;
esac
# What the viewer is actually showing. `picture=N` is how many layers the
# compositor had a decoded picture for and `canvas=lit` says the canvas it drew
# is not black. The window holds its ready line until its decoders have landed,
# so this reads the picture that was photographed rather than one that arrived
# afterwards (TASK-144).
picture_count=$(printf '%s\n' "$ready_line" | sed -n 's/.*picture=\([0-9]*\).*/\1/p')
canvas_state=$(printf '%s\n' "$ready_line" | sed -n 's/.*canvas=\([a-z]*\).*/\1/p')
echo "ui-smoke: viewer showing ${picture_count:-?} decoded layer(s), canvas ${canvas_state:-?}"
if [ "$require_picture" -eq 1 ]; then
    if ! [ "${picture_count:-0}" -ge 1 ] 2>/dev/null; then
        echo "ui-smoke: the viewer composited no decoded picture; the screenshot would be a black canvas" >&2
        exit 1
    fi
    if [ "${canvas_state:-black}" != "lit" ]; then
        echo "ui-smoke: the compositor's canvas is black" >&2
        exit 1
    fi
    echo "ui-smoke: the viewer is showing real decoded picture"
fi

# Where the two windows actually are, read back off the server rather than
# inferred from the position the app was asked for. Without a window manager
# every top-level window is a direct child of the root, so the last "+x+y" on
# an xwininfo tree line is its absolute origin on the desktop.
xwininfo -display "$display" -root -tree >"$out_dir/windows.txt" 2>&1 || true

# window_geometry TITLE -> "W H X Y" on stdout, empty if there is no such
# window. The colon after the quoted title is what keeps "Subordinate" from
# matching "Subordinate viewer".
window_geometry() {
    awk -v want="\"$1\":" '
        index($0, want) == 0 || found { next }
        {
            # "... 1280x800+0+0  +1280+0": the last two fields are the size
            # with its offset inside its parent, then the absolute origin.
            size = $(NF - 1)
            origin = $NF
            if (size !~ /^[0-9]+x[0-9]+/) { next }
            sub(/\+.*$/, "", size)
            split(size, wh, "x")
            n = split(substr(origin, 2), xy, "+")
            if (n < 2) { next }
            print wh[1], wh[2], xy[1], xy[2]
            found = 1
        }' "$out_dir/windows.txt"
}

editor_geom=$(window_geometry "Subordinate")
popout_geom=$(window_geometry "Subordinate viewer")
echo "ui-smoke: editor window  ${editor_geom:-<not found>}"
echo "ui-smoke: pop-out window ${popout_geom:-<not found>}"

# head_of X Y -> the index of the head whose rectangle contains that point, or
# nothing. The heads are the same list the crops below are cut from, so a
# window said to be on head 1 is a window inside screen-1.png.
head_of() {
    printf '%s\n' "$heads" | awk -v px="$1" -v py="$2" '
        px >= $3 && px < $3 + $1 && py >= $4 && py < $4 + $2 { print NR - 1; exit }'
}

popout_head=""
if [ -n "$popout_geom" ]; then
    popout_head=$(head_of "$(echo "$popout_geom" | awk '{print $3}')" \
        "$(echo "$popout_geom" | awk '{print $4}')")
fi
editor_head=""
if [ -n "$editor_geom" ]; then
    editor_head=$(head_of "$(echo "$editor_geom" | awk '{print $3}')" \
        "$(echo "$editor_geom" | awk '{print $4}')")
fi
echo "ui-smoke: editor on head ${editor_head:-?}, pop-out on head ${popout_head:-?}"

# outline_window PNG HEAD_X HEAD_Y "W H X Y" LABEL
#
# Draws the window's rectangle and names it on the head's screenshot. Both
# windows paint the picture on black and the desktop behind them is black, so
# an unannotated capture of the pop-out is a black rectangle on a black field:
# the window is there, and the picture says nothing to the eye. The outline is
# the evidence (TASK-118).
#
# box has no configured ImageMagick font and `-annotate` is fatal without one
# (run 34618215215), so the text is drawn only when a TrueType file can be
# found, and the whole annotation is best-effort: a screenshot is worth more
# than a label.
label_font=$(find /usr/share/fonts -name 'DejaVuSans.ttf' -print -quit 2>/dev/null || true)
[ -n "$label_font" ] ||
    label_font=$(find /usr/share/fonts -name '*.ttf' -print -quit 2>/dev/null || true)

outline_window() {
    png=$1
    head_x=$2
    head_y=$3
    geom=$4
    label=$5
    [ -n "$geom" ] || return 0
    read -r w h x y <<EOF2
$geom
EOF2
    rx=$((x - head_x))
    ry=$((y - head_y))
    set -- "$png" -stroke '#ff4d3d' -strokewidth 3 -fill none \
        -draw "rectangle $rx,$ry $((rx + w - 1)),$((ry + h - 1))"
    if [ -n "$label_font" ]; then
        set -- "$@" -stroke none -fill '#ff4d3d' -font "$label_font" -pointsize 20 \
            -annotate "+$((rx + 10))+$((ry + 34))" "$label ${w}x$h"
    fi
    im "$@" "$png" || echo "ui-smoke: could not outline $label on $png"
}

# One capture of the whole Xinerama desktop, then a crop per head: two xwd
# runs could catch the two windows a frame apart.
xwd -display "$display" -root -silent >"$out_dir/root.xwd"
index=0
: >"$out_dir/screens.txt"
while read -r width height x y; do
    [ -n "$width" ] || continue
    png="$out_dir/screen-$index.png"
    im "xwd:$out_dir/root.xwd" -crop "${width}x${height}+${x}+${y}" +repage "$png"
    if [ "${editor_head:-}" = "$index" ]; then
        outline_window "$png" "$x" "$y" "$editor_geom" "Subordinate (editor)"
    fi
    if [ "${popout_head:-}" = "$index" ]; then
        outline_window "$png" "$x" "$y" "$popout_geom" "Subordinate viewer (pop-out)"
    fi
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
    echo "| window | geometry (WxH, origin) | head |"
    echo "| --- | --- | --- |"
    echo "| Subordinate (editor) | ${editor_geom:-not found} | ${editor_head:-unknown} |"
    echo "| Subordinate viewer (pop-out) | ${popout_geom:-not found} | ${popout_head:-unknown} |"
    echo
    echo "Head 0 is the editor window, head 1 the pop-out viewer."
    echo
    echo "Project: \`$project\`"
    echo
    echo "The viewer composited ${picture_count:-?} decoded layer(s); its canvas is ${canvas_state:-unknown}."
    if [ -s "$out_dir/monitors.txt" ]; then
        echo
        echo "Outputs the X server reports:"
        echo
        echo '```'
        cat "$out_dir/monitors.txt"
        echo '```'
    fi
    if [ -n "$adapter_line" ]; then
        echo
        echo "\`$adapter_line\`"
    fi
    if [ "$force_software" -eq 0 ] && [ "$software" -eq 1 ]; then
        echo
        echo "The real adapter could not present on this display (Xvfb has no"
        echo "DRI3), so the windows were drawn by the software rasteriser."
    fi
    echo
    echo '```'
    echo "$ready_line"
    echo '```'
} >"$out_dir/summary.md"

cat "$out_dir/screens.txt"
echo "ui-smoke: wrote $out_dir"

# Asserted last, after the pictures and the summary are on disk: a run that
# fails here is exactly the one whose screenshots someone wants to look at.
if [ -n "$require_popout_head" ]; then
    if [ -z "$popout_geom" ]; then
        echo "ui-smoke: no pop-out window on the server; see windows.txt" >&2
        exit 1
    fi
    if [ "${popout_head:-none}" != "$require_popout_head" ]; then
        echo "ui-smoke: the pop-out is on head ${popout_head:-none}, expected head $require_popout_head" >&2
        exit 1
    fi
    if [ -n "$editor_head" ] && [ "$editor_head" = "$require_popout_head" ]; then
        echo "ui-smoke: the editor window is on head $editor_head too; the two windows share a head" >&2
        exit 1
    fi
    echo "ui-smoke: pop-out confirmed on head $require_popout_head, editor on head ${editor_head:-?}"
fi
