#!/usr/bin/env bash
# Generate the deterministic test media fixtures used by the Rust test suite.
#
# Media binaries are never committed; every developer and CI runner synthesises
# an identical set with GStreamer instead. Output lands in fixtures/ (gitignored)
# together with a manifest.json describing every fixture.
#
# Requires gst-launch-1.0 from gstreamer1.0-tools plus the base, good, ugly and
# bad plugin sets (see docs/DEVELOPMENT.md). The lossy audio fixtures also want
# lamemp3enc, avenc_aac and vorbisenc; where one of those is absent that fixture
# is skipped rather than failing the run. Keep this script and
# scripts/gen-fixtures.ps1 in sync.
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
out_dir="$repo_root/fixtures"
include_long=0
force=0
dry_run=0
list_only=0

usage() {
    cat <<'EOF'
Usage: scripts/gen-fixtures.sh [options]

  --out DIR    write fixtures to DIR (default: <repo>/fixtures)
  --long       also generate the 10-minute long-GOP clip (slow; skipped in CI)
  --force      regenerate files that already exist
  --list       list the fixture catalogue and exit
  --dry-run    print the pipelines instead of running them
  -h, --help   show this help
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
    --out)
        [ $# -ge 2 ] || {
            echo "gen-fixtures: --out needs a directory" >&2
            exit 2
        }
        out_dir=$2
        shift 2
        ;;
    --long) include_long=1 && shift ;;
    --force) force=1 && shift ;;
    --list) list_only=1 && shift ;;
    --dry-run) dry_run=1 && shift ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        echo "gen-fixtures: unknown option '$1'" >&2
        usage >&2
        exit 2
        ;;
    esac
done

# ---------------------------------------------------------------- catalogue --
#
# One record per fixture, tab separated:
#   name  kind  width  height  duration_ns  fps_num  fps_den  vfr  lossy  long
#   description
#
# duration_ns is an exact integer count of nanoseconds; frame rates are exact
# rationals. Nothing here is ever expressed as a float.
#
# lossy marks a fixture whose codec cannot reproduce its input samples. A lossy
# encoder also adds priming and padding frames, so duration_ns is the authored
# length, which the file itself only matches to within a few tens of
# milliseconds; tests give lossy fixtures a tolerance and keep the lossless
# ones exact.
#
# 29.97 drop-frame: 300 frames at 30000/1001 is 300 * 1001 / 30000 s exactly,
# i.e. 10_010_000_000 ns.
# VFR: 90 frames at 30/1 (3 s) followed by 180 frames at 60/1 (3 s) = 6 s.
catalogue=$(
    cat <<'EOF'
bars_1080p_h264.mp4	video	1920	1080	5000000000	25	1	false	false	false	1080p SMPTE colour bars, H.264, timecode burn-in
bars_2160p_h264.mp4	video	3840	2160	5000000000	25	1	false	false	false	4K SMPTE colour bars, H.264, timecode burn-in
dropframe_2997_h264.mp4	video	1920	1080	10010000000	30000	1001	false	false	false	29.97 drop-frame clip with drop-frame timecode burn-in
vfr_60_30.mkv	video	1280	720	6000000000	60	1	true	false	false	Variable-frame-rate clip: 3 s at 30 fps then 3 s at 60 fps
longgop_720p_10min.mp4	video	1280	720	600000000000	25	1	false	false	true	10-minute long-GOP H.264 clip (250-frame GOP, B-frames)
tone_48k_stereo.wav	audio	0	0	5000000000	0	1	false	false	false	Audio only: 5 s 440 Hz sine, 48 kHz stereo, 16-bit WAV
tone_48k_stereo.flac	audio	0	0	5000000000	0	1	false	false	false	Audio only: 5 s 440 Hz sine, 48 kHz stereo, FLAC
tone_48k_stereo.mp3	audio	0	0	5000000000	0	1	false	true	false	Audio only: 5 s 440 Hz sine, 48 kHz stereo, MP3 at 192 kbit/s CBR
tone_48k_stereo.m4a	audio	0	0	5000000000	0	1	false	true	false	Audio only: 5 s 440 Hz sine, 48 kHz stereo, AAC-LC in MP4
tone_48k_stereo.ogg	audio	0	0	5000000000	0	1	false	true	false	Audio only: 5 s 440 Hz sine, 48 kHz stereo, Ogg Vorbis
EOF
)

if [ "$list_only" -eq 1 ]; then
    printf '%-26s %-6s %-9s %-5s %-6s %s\n' NAME KIND DURATION VFR LOSSY DESCRIPTION
    while IFS=$'\t' read -r name kind _w _h dur_ns _fn _fd vfr lossy long desc; do
        [ -n "$name" ] || continue
        suffix=""
        [ "$long" = "true" ] && suffix=" (--long only)"
        printf '%-26s %-6s %6s ms %-5s %-6s %s%s\n' \
            "$name" "$kind" "$((dur_ns / 1000000))" "$vfr" "$lossy" "$desc" "$suffix"
    done <<<"$catalogue"
    exit 0
fi

# --------------------------------------------------------------- preflight --
run() {
    if [ "$dry_run" -eq 1 ]; then
        printf 'gst-launch-1.0 %s\n' "$*"
    else
        gst-launch-1.0 -q -e "$@"
    fi
}

if [ "$dry_run" -eq 0 ]; then
    command -v gst-launch-1.0 >/dev/null 2>&1 || {
        echo "gen-fixtures: gst-launch-1.0 not found; install gstreamer1.0-tools" >&2
        echo "gen-fixtures: see docs/DEVELOPMENT.md for per-OS instructions" >&2
        exit 1
    }
    missing=""
    for element in videotestsrc audiotestsrc timecodestamper timeoverlay \
        capssetter concat audioconvert x264enc h264parse mp4mux matroskamux wavenc flacenc; do
        gst-inspect-1.0 --exists "$element" || missing="$missing $element"
    done
    if [ -n "$missing" ]; then
        echo "gen-fixtures: missing GStreamer elements:$missing" >&2
        echo "gen-fixtures: install the base, good, bad and ugly plugin sets" >&2
        exit 1
    fi
fi

mkdir -p "$out_dir"

# Skips a fixture whose file is already present unless --force was given.
needs() {
    local target="$out_dir/$1"
    if [ "$force" -eq 0 ] && [ -s "$target" ]; then
        echo "gen-fixtures: keeping existing $1"
        return 1
    fi
    echo "gen-fixtures: generating $1"
    return 0
}

# Colour bars with a burnt-in timecode. $1 name, $2 width, $3 height,
# $4 frame count, $5 fps numerator, $6 fps denominator. The GOP is one second
# of frames, so these clips are short-GOP and cheap to seek in.
gen_bars() {
    local name=$1 width=$2 height=$3 frames=$4 fps_n=$5 fps_d=$6
    local font_size=$((height / 16))
    local key_int=$((fps_n / fps_d))
    run videotestsrc pattern=smpte num-buffers="$frames" \
        ! "video/x-raw,format=I420,width=$width,height=$height,framerate=$fps_n/$fps_d" \
        ! timecodestamper post-messages=false \
        ! timeoverlay time-mode=time-code halignment=center valignment=bottom \
        font-desc="Monospace $font_size" \
        ! x264enc bitrate=12000 key-int-max="$key_int" speed-preset=veryfast \
        ! h264parse ! mp4mux ! filesink location="$out_dir/$name"
}

gen_video() {
    case "$1" in
    bars_1080p_h264.mp4) gen_bars "$1" 1920 1080 125 25 1 ;;
    bars_2160p_h264.mp4) gen_bars "$1" 3840 2160 125 25 1 ;;
    dropframe_2997_h264.mp4)
        # timecodestamper flags the timecode drop-frame automatically for
        # 30000/1001, which is what makes this a genuine 29.97 DF fixture.
        gen_bars "$1" 1920 1080 300 30000 1001
        ;;
    vfr_60_30.mkv)
        # Two segments with genuinely different frame spacing are concatenated
        # and relabelled to a single caps, so the encoder sees one stream while
        # the muxer records buffer timestamps 33.3 ms apart for the first three
        # seconds and 16.6 ms apart for the last three. Matroska stores those
        # per-frame timestamps, giving a real variable-frame-rate file.
        #
        # bframes=0 is load bearing: x264's reorder delay assumes one frame
        # duration, so with B-frames the pictures either side of the rate
        # change come out of the encoder carrying each other's timestamps --
        # two duplicate stamps near the end of the file and none at all for the
        # first two pictures of the faster half -- and nothing downstream can
        # then name those pictures by time. Only this fixture needs it; the
        # constant-rate clips keep their B-frames.
        run concat name=c \
            ! capssetter replace=true \
            caps="video/x-raw,format=I420,width=1280,height=720,framerate=60/1" \
            ! x264enc bitrate=6000 key-int-max=60 speed-preset=veryfast bframes=0 \
            ! h264parse ! matroskamux ! filesink location="$out_dir/$1" \
            videotestsrc pattern=smpte num-buffers=90 \
            ! "video/x-raw,format=I420,width=1280,height=720,framerate=30/1" ! c. \
            videotestsrc pattern=ball num-buffers=180 \
            ! "video/x-raw,format=I420,width=1280,height=720,framerate=60/1" ! c.
        ;;
    longgop_720p_10min.mp4)
        run videotestsrc pattern=ball num-buffers=15000 \
            ! "video/x-raw,format=I420,width=1280,height=720,framerate=25/1" \
            ! timecodestamper post-messages=false \
            ! timeoverlay time-mode=time-code halignment=center valignment=bottom \
            font-desc="Monospace 45" \
            ! x264enc bitrate=4000 key-int-max=250 bframes=3 speed-preset=veryfast \
            ! h264parse ! mp4mux ! filesink location="$out_dir/$1"
        ;;
    *)
        echo "gen-fixtures: no pipeline for '$1'" >&2
        exit 1
        ;;
    esac
}

# The encoder tail of an audio fixture's pipeline, one element per word. The
# lossless formats need a single encoder; the lossy ones need an encoder and,
# for AAC, a parser and a container.
audio_encoder_chain() {
    case "$1" in
    *.wav) echo "wavenc" ;;
    *.flac) echo "flacenc" ;;
    *.mp3) echo "lamemp3enc:target=bitrate:bitrate=192:cbr=true" ;;
    *.m4a) echo "avenc_aac:bitrate=192000 aacparse mp4mux" ;;
    *.ogg) echo "vorbisenc:quality=0.6 oggmux" ;;
    *)
        echo "gen-fixtures: no pipeline for '$1'" >&2
        exit 1
        ;;
    esac
}

# True when every element an audio fixture needs is installed. MP3, AAC and
# Ogg Vorbis encoders live in plugin sets a minimal install may not carry, so a
# missing one skips that fixture instead of failing the whole run.
audio_encoder_available() {
    [ "$dry_run" -eq 1 ] && return 0
    local step element
    for step in $(audio_encoder_chain "$1"); do
        element=${step%%:*}
        gst-inspect-1.0 --exists "$element" || return 1
    done
    return 0
}

# 5 s at 48 kHz with 4800 samples per buffer is exactly 50 buffers. Lossy
# encoders add priming and padding around those 240000 frames, so the file is
# a little longer than the catalogue's authored duration.
gen_audio() {
    local step args=()
    for step in $(audio_encoder_chain "$1"); do
        [ ${#args[@]} -eq 0 ] || args+=('!')
        # "element:prop=value:prop=value" carries its properties as extra words.
        # shellcheck disable=SC2206 # deliberate word splitting
        args+=(${step//:/ })
    done
    run audiotestsrc wave=sine freq=440 samplesperbuffer=4800 num-buffers=50 \
        ! "audio/x-raw,format=S16LE,rate=48000,channels=2" \
        ! audioconvert ! "${args[@]}" ! filesink location="$out_dir/$1"
}

# ---------------------------------------------------------------- generate --
manifest_entries=""
while IFS=$'\t' read -r name kind width height dur_ns fps_n fps_d vfr lossy long desc; do
    [ -n "$name" ] || continue
    generated=true
    if [ "$long" = "true" ] && [ "$include_long" -eq 0 ]; then
        echo "gen-fixtures: skipping $name (pass --long to generate it)"
        generated=false
    elif [ "$kind" = audio ] && ! audio_encoder_available "$name"; then
        echo "gen-fixtures: skipping $name (its encoder is not installed)"
        generated=false
    elif needs "$name"; then
        case "$kind" in
        video) gen_video "$name" ;;
        audio) gen_audio "$name" ;;
        esac
    fi
    if [ "$dry_run" -eq 0 ] && [ "$generated" = true ] && [ ! -s "$out_dir/$name" ]; then
        echo "gen-fixtures: $name was not written" >&2
        exit 1
    fi
    manifest_entries="$manifest_entries
    {
      \"name\": \"$name\",
      \"kind\": \"$kind\",
      \"width\": $width,
      \"height\": $height,
      \"duration_ns\": $dur_ns,
      \"fps_num\": $fps_n,
      \"fps_den\": $fps_d,
      \"vfr\": $vfr,
      \"lossy\": $lossy,
      \"generated\": $generated,
      \"description\": \"$desc\"
    },"
done <<<"$catalogue"

if [ "$dry_run" -eq 1 ]; then
    echo "gen-fixtures: dry run, manifest not written"
    exit 0
fi

# Trim the trailing comma of the last entry so the JSON is valid.
manifest_entries=${manifest_entries%,}
cat >"$out_dir/manifest.json" <<EOF
{
  "version": 1,
  "generator": "scripts/gen-fixtures.sh",
  "fixtures": [$manifest_entries
  ]
}
EOF
echo "gen-fixtures: wrote $out_dir/manifest.json"
