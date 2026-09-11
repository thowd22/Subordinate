#!/usr/bin/env bash
# Build the Subordinate AppImage with the pinned GStreamer runtime inside it.
#
# The package has to run on a stock Ubuntu LTS and a stock Fedora with nothing
# installed beyond a graphics driver, so every GStreamer library and plugin is
# copied in. The host keeps only the driver stack (Mesa/NVIDIA), the display
# server and the audio daemon -- bundling those is what breaks hardware
# acceleration, so the excludelist below is deliberate, not incidental.
#
# Usage:
#   packaging/linux/build-appimage.sh [options]
#
#   --stage-only        assemble the AppDir and stop (no appimagetool, no FUSE)
#   --skip-build        reuse the release binaries already in target/release
#   --gst-prefix DIR    GStreamer install prefix to bundle from
#                       (default: the prefix pkg-config reports, or $GST_PREFIX)
#   --out DIR           where the AppDir and .AppImage land (default target/appimage)
#   -h, --help          this text
#
# On this project's sudo-less dev boxes point --gst-prefix at the extracted
# runtime, e.g. --gst-prefix "$GSTROOT/usr". In CI the default is right.
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
packaging_dir=$repo_root/packaging/linux
app_id=io.github.thowd22.Subordinate

stage_only=0
skip_build=0
gst_prefix=${GST_PREFIX:-}
out_dir=$repo_root/target/appimage

usage() {
    sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --stage-only) stage_only=1 ;;
        --skip-build) skip_build=1 ;;
        --gst-prefix) gst_prefix=${2:?--gst-prefix needs a directory}; shift ;;
        --out) out_dir=${2:?--out needs a directory}; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "build-appimage.sh: unknown option $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

die() { echo "build-appimage.sh: $*" >&2; exit 1; }

version=$(sed -n '/^\[workspace.package\]/,/^\[/p' "$repo_root/Cargo.toml" |
    sed -n 's/^version = "\(.*\)"/\1/p' | head -n1)
[ -n "$version" ] || die "could not read the workspace version from Cargo.toml"

# ---------------------------------------------------------------------------
# Locate the GStreamer runtime to bundle.
# ---------------------------------------------------------------------------
if [ -z "$gst_prefix" ]; then
    command -v pkg-config >/dev/null 2>&1 || die "no pkg-config and no --gst-prefix"
    gst_libdir=$(pkg-config --variable=libdir gstreamer-1.0) ||
        die "pkg-config cannot find gstreamer-1.0; install it or pass --gst-prefix"
else
    # lib/gstreamer-1.0 on Fedora and Arch, lib/<triplet>/gstreamer-1.0 on
    # Debian and Ubuntu; include/ and share/ carry same-named directories, so
    # the path filter matters.
    gst_libdir=$(find "$gst_prefix" -maxdepth 4 -type d -name 'gstreamer-1.0' \
        -path '*/lib*' -printf '%h\n' -quit 2>/dev/null || true)
    [ -n "$gst_libdir" ] || die "no */lib*/gstreamer-1.0 under $gst_prefix"
fi
gst_plugin_dir=$gst_libdir/gstreamer-1.0
[ -d "$gst_plugin_dir" ] || die "no GStreamer plugin directory at $gst_plugin_dir"
gst_bindir=$(cd "$gst_libdir/../../bin" 2>/dev/null && pwd || true)
[ -n "$gst_bindir" ] || gst_bindir=$(cd "$gst_libdir/../bin" 2>/dev/null && pwd || true)

echo "==> version      $version"
echo "==> gst libdir   $gst_libdir"
echo "==> plugin dir   $gst_plugin_dir"

# ---------------------------------------------------------------------------
# Build the binaries.
# ---------------------------------------------------------------------------
if [ "$skip_build" -eq 0 ]; then
    echo "==> cargo build --release"
    (cd "$repo_root" && cargo build --release -p subordinate -p subordinate-cli)
fi
bin_gui=$repo_root/target/release/subordinate
bin_cli=$repo_root/target/release/subordinate-cli
[ -x "$bin_gui" ] || die "missing $bin_gui (drop --skip-build?)"

# ---------------------------------------------------------------------------
# Stage the AppDir.
# ---------------------------------------------------------------------------
appdir=$out_dir/Subordinate.AppDir
rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/lib/gstreamer-1.0" \
    "$appdir/usr/share/applications" \
    "$appdir/usr/share/metainfo" \
    "$appdir/usr/share/icons/hicolor/scalable/apps"

install -m 0755 "$bin_gui" "$appdir/usr/bin/subordinate"
# The CLI rides along so a package can be scripted (and so hardware.yml can
# render from inside it); it is optional because --skip-build may be pointed at
# a tree where only the editor was built.
if [ -x "$bin_cli" ]; then
    install -m 0755 "$bin_cli" "$appdir/usr/bin/subordinate-cli"
else
    echo "==> subordinate-cli not built; the package will be GUI only"
fi
install -m 0755 "$packaging_dir/AppRun" "$appdir/AppRun"

install -m 0644 "$packaging_dir/subordinate.desktop" \
    "$appdir/usr/share/applications/$app_id.desktop"
install -m 0644 "$packaging_dir/$app_id.metainfo.xml" \
    "$appdir/usr/share/metainfo/$app_id.metainfo.xml"
install -m 0644 "$packaging_dir/subordinate.svg" \
    "$appdir/usr/share/icons/hicolor/scalable/apps/$app_id.svg"
# appimagetool insists on a .desktop and an icon at the AppDir root.
ln -sf "usr/share/applications/$app_id.desktop" "$appdir/$app_id.desktop"
ln -sf "usr/share/icons/hicolor/scalable/apps/$app_id.svg" "$appdir/$app_id.svg"
ln -sf "$app_id.svg" "$appdir/.DirIcon"

# --- plugins ---------------------------------------------------------------
# A `!` in the allowlist means the package cannot ship without that plugin; a
# plain entry is copied when the runtime has it. So an image without msdk is
# fine, one without nvcodec is a build failure.
missing_optional=()
while read -r plugin; do
    plugin=${plugin%%#*}
    plugin=$(echo "$plugin" | tr -d '[:space:]')
    [ -n "$plugin" ] || continue
    required=0
    if [ "${plugin:0:1}" = '!' ]; then
        required=1
        plugin=${plugin:1}
    fi
    so=$gst_plugin_dir/libgst$plugin.so
    if [ -f "$so" ]; then
        install -m 0644 "$so" "$appdir/usr/lib/gstreamer-1.0/"
    elif [ "$required" -eq 1 ]; then
        die "required plugin $plugin is not in $gst_plugin_dir; this runtime cannot be shipped"
    else
        missing_optional+=("$plugin")
    fi
done < "$packaging_dir/gst-plugins.txt"

if [ ${#missing_optional[@]} -gt 0 ]; then
    echo "==> optional plugins not in this runtime: ${missing_optional[*]}"
fi
echo "==> bundled $(find "$appdir/usr/lib/gstreamer-1.0" -name '*.so' | wc -l) plugins"

# The scanner is a helper binary GStreamer execs; without it every plugin is
# loaded in-process and one bad module takes the editor down with it.
scanner=$(find "$gst_libdir" -name gst-plugin-scanner -type f -print -quit)
[ -n "$scanner" ] || die "no gst-plugin-scanner under $gst_libdir"
install -m 0755 "$scanner" "$appdir/usr/lib/gstreamer-1.0/gst-plugin-scanner"

# gst-inspect and gst-discoverer ship so the package can be inspected from the
# outside (SUB_APPIMAGE_TOOL in AppRun) -- that is how hardware encode gets
# verified against the bundled runtime rather than the host's.
for tool in gst-inspect-1.0 gst-discoverer-1.0; do
    if [ -n "$gst_bindir" ] && [ -x "$gst_bindir/$tool" ]; then
        install -m 0755 "$gst_bindir/$tool" "$appdir/usr/bin/$tool"
    else
        echo "==> $tool not found; the package will not be self-inspectable"
    fi
done

# --- shared libraries ------------------------------------------------------
# Never bundled: the loader and libc family (bundling those is what makes an
# AppImage refuse to start on a newer host), the C++ runtime, and every library
# that talks to kernel or daemon state on the host -- graphics drivers, VA-API,
# CUDA, the display server, the audio daemon.
exclude_re='^(ld-linux.*|libc|libm|libdl|libpthread|librt|libresolv|libutil|libnsl|libanl|libstdc\+\+|libgcc_s|libGL|libGLX|libGLdispatch|libEGL|libGLESv2|libOpenGL|libgbm|libdrm|libva|libva-drm|libva-x11|libvdpau|libvulkan|libcuda|libnvcuvid|libnvidia-encode|libnvidia-.*|libX11|libX11-xcb|libxcb.*|libXext|libXfixes|libXrender|libXrandr|libXi|libXcursor|libXinerama|libXau|libXdmcp|libwayland-.*|libasound|libpulse.*|libpipewire.*|libjack.*|libdbus-1|libsystemd|libudev|libselinux)\.so.*$'

copy_dependencies() {
    local target=$1
    ldd "$target" 2>/dev/null | awk '/=> \//{print $3}' | while read -r lib; do
        local base
        base=$(basename "$lib")
        [[ $base =~ $exclude_re ]] && continue
        [ -f "$appdir/usr/lib/$base" ] && continue
        install -m 0644 "$lib" "$appdir/usr/lib/$base"
    done
}

echo "==> resolving shared libraries"
# Two passes: libraries pulled in by a bundled library itself (GStreamer's
# plugins drag in orc, glib, libav, x264...) only show up once that library is
# in the AppDir.
copy_dependencies "$appdir/usr/bin/subordinate"
for pass in 1 2 3; do
    before=$(find "$appdir/usr/lib" -maxdepth 1 -name '*.so*' | wc -l)
    while read -r lib; do
        copy_dependencies "$lib"
    done < <(find "$appdir/usr/lib" -maxdepth 2 -name '*.so*' -type f)
    after=$(find "$appdir/usr/lib" -maxdepth 1 -name '*.so*' | wc -l)
    [ "$before" = "$after" ] && break
    echo "==> pass $pass: $before -> $after libraries"
done
echo "==> bundled $(find "$appdir/usr/lib" -maxdepth 1 -name '*.so*' | wc -l) libraries"

# --- plugins whose dependencies are not in the bundle -----------------------
# A plugin with an unsatisfiable dependency does not fail loudly: GStreamer
# blacklists it at scan time, prints a warning nobody reads and carries on, so
# the package ships an element that is simply absent. Some of those deps are
# dlopen-only libraries the runtime image never had (libmfx for msdk,
# libopenh264), which is fine for an optional plugin and fatal for a required
# one. Drop the optional ones here so the registry stays clean.
dropped=()
while read -r so; do
    unresolved=$(LD_LIBRARY_PATH=$appdir/usr/lib ldd "$so" 2>/dev/null |
        awk '/not found/{print $1}' | tr '\n' ' ')
    [ -n "$unresolved" ] || continue
    name=$(basename "$so" .so)
    name=${name#libgst}
    if grep -qx "!$name" <(sed 's/#.*//' "$packaging_dir/gst-plugins.txt" | tr -d ' \t'); then
        die "required plugin $name cannot resolve: $unresolved"
    fi
    rm -f "$so"
    dropped+=("$name ($unresolved)")
done < <(find "$appdir/usr/lib/gstreamer-1.0" -name 'libgst*.so')
if [ ${#dropped[@]} -gt 0 ]; then
    echo "==> dropped plugins with unresolved dependencies: ${dropped[*]}"
fi

# An rpath means the binary finds its libraries even when something clears
# LD_LIBRARY_PATH. AppRun sets the variable too, so patchelf is a nicety.
if command -v patchelf >/dev/null 2>&1; then
    patchelf --set-rpath '$ORIGIN/../lib' "$appdir/usr/bin/subordinate"
    if [ -x "$appdir/usr/bin/subordinate-cli" ]; then
        patchelf --set-rpath '$ORIGIN/../lib' "$appdir/usr/bin/subordinate-cli"
    fi
else
    echo "==> patchelf missing; relying on LD_LIBRARY_PATH from AppRun"
fi

du -sh "$appdir"
if [ "$stage_only" -eq 1 ]; then
    echo "==> staged $appdir (stage-only)"
    exit 0
fi

# ---------------------------------------------------------------------------
# Pack it.
# ---------------------------------------------------------------------------
tool=${APPIMAGETOOL:-}
if [ -z "$tool" ]; then
    if command -v appimagetool >/dev/null 2>&1; then
        tool=$(command -v appimagetool)
    else
        cache=${XDG_CACHE_HOME:-$HOME/.cache}/subordinate
        mkdir -p "$cache"
        tool=$cache/appimagetool-x86_64.AppImage
        [ -x "$tool" ] || {
            echo "==> downloading appimagetool"
            curl -fsSL -o "$tool" \
                https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
            chmod +x "$tool"
        }
    fi
fi

# CI containers have no FUSE, and appimagetool is itself an AppImage;
# --appimage-extract-and-run is the supported way round that.
export ARCH=x86_64
out_file=$out_dir/Subordinate-$version-x86_64.AppImage
"$tool" --appimage-extract-and-run "$appdir" "$out_file"
echo "==> built $out_file"
