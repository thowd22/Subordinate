#!/usr/bin/env bash
# Build the Subordinate Flatpak and, unless told otherwise, export it as a
# single-file bundle.
#
# Usage:
#   packaging/flatpak/build-flatpak.sh [options]
#
#   --install-deps   flatpak install the runtime, SDK and extensions first
#   --no-bundle      build into the repo only, skip the .flatpak bundle
#   --out DIR        build tree and bundle location (default target/flatpak)
#   -h, --help       this text
#
# Needs flatpak and flatpak-builder on PATH. Everything else -- the runtime,
# the SDK and the rust-stable SDK extension -- comes from Flathub with
# --install-deps. There is no ffmpeg-full to install: since branch 25.08 the
# full codec set is org.freedesktop.Platform.codecs-extra, which the runtime
# declares and pulls in itself.
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
manifest=$repo_root/packaging/flatpak/io.github.thowd22.Subordinate.yml
app_id=io.github.thowd22.Subordinate
runtime_version=25.08

install_deps=0
bundle=1
out_dir=$repo_root/target/flatpak

usage() { sed -n '2,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
    case "$1" in
        --install-deps) install_deps=1 ;;
        --no-bundle) bundle=0 ;;
        --out) out_dir=${2:?--out needs a directory}; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "build-flatpak.sh: unknown option $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

command -v flatpak >/dev/null 2>&1 || { echo "build-flatpak.sh: flatpak is not installed" >&2; exit 1; }
command -v flatpak-builder >/dev/null 2>&1 || { echo "build-flatpak.sh: flatpak-builder is not installed" >&2; exit 1; }

if [ "$install_deps" -eq 1 ]; then
    flatpak remote-add --if-not-exists --user flathub \
        https://dl.flathub.org/repo/flathub.flatpakrepo
    flatpak install --user --noninteractive flathub \
        "org.freedesktop.Platform//$runtime_version" \
        "org.freedesktop.Sdk//$runtime_version" \
        "org.freedesktop.Sdk.Extension.rust-stable//$runtime_version"
    # The toolchain the SDK extension carries has to satisfy the workspace's
    # rust-version, or cargo stops with "rustc X is not supported by the
    # following packages" after several minutes of flatpak-builder setup.
    flatpak run --user --command=/usr/lib/sdk/rust-stable/bin/rustc \
        --devel "org.freedesktop.Sdk//$runtime_version" --version ||
        echo "build-flatpak.sh: could not query the SDK extension's rustc" >&2
fi

mkdir -p "$out_dir"
version=$(sed -n '/^\[workspace.package\]/,/^\[/p' "$repo_root/Cargo.toml" |
    sed -n 's/^version = "\(.*\)"/\1/p' | head -n1)

flatpak-builder --user --force-clean --disable-rofiles-fuse \
    --repo="$out_dir/repo" \
    "$out_dir/build" "$manifest"

if [ "$bundle" -eq 1 ]; then
    out_file=$out_dir/Subordinate-$version.flatpak
    flatpak build-bundle "$out_dir/repo" "$out_file" "$app_id" "$runtime_version"
    echo "==> built $out_file"
fi
