#!/usr/bin/env bash
# Fetch the CC0 media the sample project in examples/sample-project plays.
#
# The media is never committed: every developer and CI runner downloads the
# same three files from Wikimedia Commons instead, each pinned by URL, byte
# size and SHA-256, so a file that changed upstream fails here rather than
# quietly changing what the sample project renders. Output lands in
# examples/sample-project/media/ (gitignored) beside a manifest.json listing
# every file, its licence and where it came from.
#
# Everything in the catalogue is CC0 1.0 (public domain dedication): no
# attribution is required, and examples/sample-project/README.md credits each
# author anyway. Keep this script and scripts/get-sample-media.ps1 in sync.
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
out_dir="$repo_root/examples/sample-project/media"
force=0
list_only=0
dry_run=0

usage() {
    cat <<'EOF'
Usage: scripts/get-sample-media.sh [options]

  --out DIR    write media to DIR (default: <repo>/examples/sample-project/media)
  --force      re-download files that already exist
  --list       list the media catalogue and exit
  --dry-run    print what would be downloaded instead of downloading it
  -h, --help   show this help
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
    --out)
        [ $# -ge 2 ] || {
            echo "get-sample-media: --out needs a directory" >&2
            exit 2
        }
        out_dir=$2
        shift 2
        ;;
    --force) force=1 && shift ;;
    --list) list_only=1 && shift ;;
    --dry-run) dry_run=1 && shift ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        echo "get-sample-media: unknown option '$1'" >&2
        usage >&2
        exit 2
        ;;
    esac
done

# ---------------------------------------------------------------- catalogue --
#
# One record per file, tab separated:
#   name  bytes  sha256  url  title  author  source_page
#
# `name` is the file name the sample project refers to, which is a slug rather
# than the Commons file name: the project file has to name it on Windows too,
# and the originals carry accents, spaces and parentheses.
#
# The URLs are upload.wikimedia.org content URLs, which are stable for the
# life of a revision; the SHA-256 pins the revision itself.
catalogue=$(
    cat <<'EOF'
porters-paris-1921.webm	930338	a46b99a8d0de346fa999524c5462d64dec2cb3f3f2122b96f1190cfad2c2f02d	https://upload.wikimedia.org/wikipedia/commons/7/7d/Ancienne_et_nouvelle_tenue_des_porteurs_des_Pompes_Fun%C3%A8bres_de_la_Ville_de_Paris_-_AI49294.webm	Ancienne et nouvelle tenue des porteurs des Pompes Funebres de la Ville de Paris	Le Saint Lucien	https://commons.wikimedia.org/wiki/File:Ancienne_et_nouvelle_tenue_des_porteurs_des_Pompes_Fun%C3%A8bres_de_la_Ville_de_Paris_-_AI49294.webm
crowned-pigeon.webm	3595734	69ecc307be7b24521596ef489a952dca4145cf20f291f7b37a68c00ae10deabf	https://upload.wikimedia.org/wikipedia/commons/0/00/Victorian_Crowned_Pigeon_fighting.webm	Victorian Crowned Pigeon fighting	Designism	https://commons.wikimedia.org/wiki/File:Victorian_Crowned_Pigeon_fighting.webm
soneros-en-xalapa.webm	176564	0545e135408bd2e73a11db1e54d203fdab2432401584cdc4460052a6f352d3ec	https://upload.wikimedia.org/wikipedia/commons/b/b3/Soneros_en_Xalapa.webm	Soneros en Xalapa	Koffermejia	https://commons.wikimedia.org/wiki/File:Soneros_en_Xalapa.webm
EOF
)

# The licence every file in the catalogue carries.
licence="CC0-1.0"
licence_url="https://creativecommons.org/publicdomain/zero/1.0/"

if [ "$list_only" -eq 1 ]; then
    printf '%-24s %10s %-9s %s\n' NAME BYTES LICENCE TITLE
    while IFS=$'\t' read -r name bytes _sha _url title _author _page; do
        [ -n "$name" ] || continue
        printf '%-24s %10s %-9s %s\n' "$name" "$bytes" "$licence" "$title"
    done <<<"$catalogue"
    exit 0
fi

# ---------------------------------------------------------------- downloads --
# Wikimedia asks automated clients to identify themselves.
user_agent="Subordinate-sample-media/1.0 (scripts/get-sample-media.sh; a video editor's sample project)"

if [ "$dry_run" -eq 0 ]; then
    command -v curl >/dev/null 2>&1 || {
        echo "get-sample-media: curl not found" >&2
        exit 1
    }
fi

# Prints the SHA-256 of $1. Linux carries sha256sum, macOS shasum.
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# Downloads $2 to $1 and checks it against $3 (sha256) and $4 (bytes). A file
# that fails either check is deleted, so a rerun cannot mistake it for good.
fetch() {
    local target=$1 url=$2 want_sha=$3 want_bytes=$4
    # Wikimedia rate-limits bursts from shared CI egress IPs with HTTP 429, and
    # curl's own --retry backs off far too fast to clear one. Retry the whole
    # transfer a handful of times with a growing delay instead; the sleeps are
    # long enough that a rate limit has expired by the last attempt.
    local attempt delay=5
    for attempt in 1 2 3 4 5; do
        if curl -fsSL -A "$user_agent" --retry 2 --retry-delay 3 --retry-all-errors \
            -o "$target.part" "$url"; then
            break
        fi
        rm -f "$target.part"
        if [ "$attempt" -eq 5 ]; then
            echo "get-sample-media: giving up on $(basename "$target") after 5 attempts" >&2
            exit 1
        fi
        echo "get-sample-media: download of $(basename "$target") failed, retrying in ${delay}s" >&2
        sleep "$delay"
        delay=$((delay * 2))
    done
    local got_bytes
    got_bytes=$(wc -c <"$target.part" | tr -d ' ')
    if [ "$got_bytes" != "$want_bytes" ]; then
        rm -f "$target.part"
        echo "get-sample-media: $(basename "$target") is $got_bytes bytes, expected $want_bytes" >&2
        exit 1
    fi
    local got_sha
    got_sha=$(sha256_of "$target.part")
    if [ "$got_sha" != "$want_sha" ]; then
        rm -f "$target.part"
        echo "get-sample-media: $(basename "$target") hashes to $got_sha, expected $want_sha" >&2
        echo "get-sample-media: the file upstream is not the one this project was built against" >&2
        exit 1
    fi
    mv "$target.part" "$target"
}

mkdir -p "$out_dir"

manifest_entries=""
while IFS=$'\t' read -r name bytes sha url title author page; do
    [ -n "$name" ] || continue
    target="$out_dir/$name"
    if [ "$dry_run" -eq 1 ]; then
        echo "get-sample-media: would download $name from $url"
    elif [ "$force" -eq 0 ] && [ -s "$target" ] && [ "$(sha256_of "$target")" = "$sha" ]; then
        echo "get-sample-media: keeping existing $name"
    else
        echo "get-sample-media: downloading $name ($bytes bytes)"
        fetch "$target" "$url" "$sha" "$bytes"
    fi
    manifest_entries="$manifest_entries
    {
      \"name\": \"$name\",
      \"bytes\": $bytes,
      \"sha256\": \"$sha\",
      \"url\": \"$url\",
      \"title\": \"$title\",
      \"author\": \"$author\",
      \"source\": \"$page\",
      \"licence\": \"$licence\",
      \"licence_url\": \"$licence_url\"
    },"
done <<<"$catalogue"

if [ "$dry_run" -eq 1 ]; then
    echo "get-sample-media: dry run, manifest not written"
    exit 0
fi

# Trim the trailing comma of the last entry so the JSON is valid.
manifest_entries=${manifest_entries%,}
cat >"$out_dir/manifest.json" <<EOF
{
  "version": 1,
  "generator": "scripts/get-sample-media.sh",
  "media": [$manifest_entries
  ]
}
EOF
echo "get-sample-media: wrote $out_dir/manifest.json"
