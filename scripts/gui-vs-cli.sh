#!/usr/bin/env bash
# One GUI-versus-CLI export comparison on the Linux desktop image (TASK-143).
#
#   gui-vs-cli.sh <encoder> <project.sub> <rank>
#
# Starts the released editor on the image's real Xorg session with one element
# ranked so the automatic order lands on <encoder>, waits for it to say it has
# painted and bound its Command API, runs scripts/gui-export-compare.py against
# that window, then stops it.
#
# The rank is how the encoder is chosen because `export.render` takes no
# encoder parameter: GStreamer registers hardware encoders at rank NONE, so the
# automatic order lands on x264enc unless something lifts NVENC above it, and
# `GST_PLUGIN_FEATURE_RANK` is the supported way to do that from outside the
# process. The CLI side pins the same element with `--encoder`, and its report
# names the element that really ran.
set -euo pipefail

encoder="${1:?an encoder element}"
project="${2:?a project file}"
rank="${3:-primary}"

out="${GITHUB_WORKSPACE:-$PWD}/target/gui-vs-cli/$encoder"
mkdir -p "$out"

case "$rank" in
  primary) ranks="nvh264enc:primary" ;;
  none)    ranks="nvh264enc:none" ;;
  *)       ranks="$rank" ;;
esac

export GST_PLUGIN_FEATURE_RANK="$ranks"
echo "GST_PLUGIN_FEATURE_RANK=$GST_PLUGIN_FEATURE_RANK"

# setsid + nohup: the window has to outlive this shell's foreground job while
# the comparison drives it over the socket.
setsid nohup subordinate --ui-smoke --hold-seconds 900 "$project" \
  >"$out/app.log" 2>&1 &
app=$!
echo "editor pid $app"

ready=no
for _ in $(seq 1 180); do
  if grep -q 'ui-smoke ready' "$out/app.log" 2>/dev/null; then ready=yes; break; fi
  if ! kill -0 "$app" 2>/dev/null; then break; fi
  sleep 1
done
if [ "$ready" != yes ]; then
  echo "::error::the editor never reported ui-smoke ready with $encoder ranked"
  cat "$out/app.log" || true
  kill "$app" 2>/dev/null || true
  exit 1
fi
grep -m1 'ui-smoke ready' "$out/app.log"

status=0
python3 scripts/gui-export-compare.py \
  --cli /usr/local/bin/subordinate-cli \
  --project "$project" \
  --sequence UHD \
  --preset youtube-1080p \
  --container mp4 \
  --encoders "$encoder" \
  --frames 60 \
  --gst-prefix subordinate- \
  --out "$out" || status=$?

kill "$app" 2>/dev/null || true
wait "$app" 2>/dev/null || true
exit "$status"
