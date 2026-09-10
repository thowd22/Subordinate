#!/usr/bin/env bash
# The phase 6 exit test: one prompt, one plugin, no manual steps (TASK-102).
#
# Builds the two binaries, serves the Command API on a scratch instance with a
# plugin directory of its own, hands docs/agent-runbook.md's prompt to a fresh
# non-interactive Claude Code session with the editor's MCP tools allowed, and
# then re-checks the result itself with `subordinate-cli plugin test` against
# docs/examples/gaps.sub -- so the verdict is the harness's, not the agent's.
#
# The prompt is not written here. It is read from the runbook between its
# <!-- prompt:start --> markers and rendered, so the document an author reads
# and the message the agent receives are the same text.
#
# Requires: a Rust toolchain with wasm32-wasip2, a GStreamer development
# install (docs/DEVELOPMENT.md), and the `claude` CLI on PATH.
# Output: <work>/prompt.txt, <work>/transcript.json, <work>/verify.json,
#         <work>/remove-gaps/ and one JSON verdict on stdout.
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
runbook="$repo_root/docs/agent-runbook.md"
fixture_source="$repo_root/docs/examples/gaps.sub"
plugin_id="com.example.remove-gaps"
profile="debug"
build="yes"
work=""
agent=""
mode="run"
keep_work="no"

# The tools the session may use without being asked: the editor's whole MCP
# surface, and the file and shell tools it needs to write Rust and build it.
allowed_tools="mcp__subordinate,Bash,Read,Write,Edit,Glob,Grep"

usage() {
    cat <<'EOF'
Usage: scripts/agent-runbook.sh [options]

  --work DIR     run in DIR instead of a fresh temporary directory
  --release      build the binaries in release
  --no-build     use the binaries already in target/, building nothing
  --agent CMD    the agent to drive; the prompt arrives on stdin and the MCP
                 config path is in $SUBORDINATE_MCP_CONFIG
                 (default: claude -p with the editor's tools allowed)
  --prompt       print the rendered prompt and stop
  --verify-only  skip the session and only re-check what is in --work
  --keep         keep the work directory even when it was a temporary one
  -h, --help     this text
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --work) work="$2"; keep_work="yes"; shift 2 ;;
        --release) profile="release"; shift ;;
        --no-build) build="no"; shift ;;
        --agent) agent="$2"; shift 2 ;;
        --prompt) mode="prompt"; shift ;;
        --verify-only) mode="verify"; shift ;;
        --keep) keep_work="yes"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

die() { echo "agent-runbook: $*" >&2; exit 1; }

# --- the prompt -------------------------------------------------------------

# The runbook's prompt block, with the run's paths substituted in.
render_prompt() {
    local workdir="$1" fixture="$2" text
    text=$(awk '/<!-- prompt:start -->/ {inside=1; next}
                /<!-- prompt:end -->/ {inside=0}
                inside && !/^```/ {print}' "$runbook")
    [ -n "$text" ] || die "no prompt block in $runbook"
    text=${text//\{\{WORKDIR\}\}/$workdir}
    text=${text//\{\{SDK\}\}/$repo_root/sdk/subordinate-sdk}
    text=${text//\{\{FIXTURE\}\}/$fixture}
    printf '%s\n' "$text"
}

if [ "$mode" = "prompt" ]; then
    render_prompt "${work:-<work>}" "${work:-<work>}/gaps.sub"
    exit 0
fi

# --- preconditions ----------------------------------------------------------

[ -f "$fixture_source" ] || die "the fixture $fixture_source is missing"
command -v cargo >/dev/null || die "cargo is not on PATH"
rustup target list --installed 2>/dev/null | grep -qx wasm32-wasip2 \
    || die "the wasm32-wasip2 target is not installed: rustup target add wasm32-wasip2"
if [ "$mode" = "run" ] && [ -z "$agent" ]; then
    command -v claude >/dev/null \
        || die "the claude CLI is not on PATH; pass --agent to drive another agent"
fi

if [ -z "$work" ]; then
    work=$(mktemp -d "${TMPDIR:-/tmp}/agent-runbook-XXXXXX")
fi
mkdir -p "$work"
work=$(cd -- "$work" && pwd)
plugin_dir="$work/plugins"
endpoint_dir="$work/endpoint"
fixture="$work/gaps.sub"
mkdir -p "$plugin_dir" "$endpoint_dir"
cp "$fixture_source" "$fixture"

# A scratch instance name, so the run never adopts a real editor's endpoint.
# Kept short: with the directory below it this becomes a Unix-domain socket
# path, and macOS caps those at about a hundred bytes.
instance="runbook-$$"

# --- the binaries -----------------------------------------------------------

if [ "$build" = "yes" ]; then
    build_args=(-p subordinate-cli -p subordinate-mcp)
    [ "$profile" = "release" ] && build_args+=(--release)
    echo "agent-runbook: building subordinate-cli and subordinate-mcp" >&2
    (cd -- "$repo_root" && cargo build "${build_args[@]}" >&2)
fi
cli="$repo_root/target/$profile/subordinate-cli"
mcp="$repo_root/target/$profile/subordinate-mcp"
[ -x "$cli" ] || die "no subordinate-cli at $cli"
[ -x "$mcp" ] || die "no subordinate-mcp at $mcp"

# --- the editor -------------------------------------------------------------

server_pid=""
stop_server() {
    if [ -n "$server_pid" ]; then
        # `serve` stops when stdin reaches end of file, and closing the write
        # end of the fifo is that. Killing it is the fallback for a server that
        # is already wedged.
        exec 9>&- 2>/dev/null || true
        local waited=0
        while kill -0 "$server_pid" 2>/dev/null && [ "$waited" -lt 50 ]; do
            sleep 0.2
            waited=$((waited + 1))
        done
        kill "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
        server_pid=""
    fi
    if [ "$keep_work" = "no" ]; then
        rm -rf -- "$work"
    fi
}
trap stop_server EXIT

start_server() {
    local fifo="$work/serve.stdin"
    rm -f -- "$fifo"
    mkfifo "$fifo"
    "$cli" serve --instance "$instance" --directory "$endpoint_dir" \
        --plugin-dir "$plugin_dir" --compact <"$fifo" >"$work/serve.json" \
        2>"$work/serve.log" &
    server_pid=$!
    # Holding the write end open is what keeps the server alive.
    exec 9>"$fifo"
    local waited=0
    while [ ! -s "$work/serve.json" ]; do
        kill -0 "$server_pid" 2>/dev/null || die "the editor did not start; see $work/serve.log"
        sleep 0.2
        waited=$((waited + 1))
        [ "$waited" -lt 150 ] || die "the editor did not become ready; see $work/serve.log"
    done
}

# --- the run ----------------------------------------------------------------

if [ "$mode" = "run" ]; then
    render_prompt "$work" "$fixture" >"$work/prompt.txt"
    cat >"$work/mcp.json" <<EOF
{
  "mcpServers": {
    "subordinate": {
      "type": "stdio",
      "command": "$mcp",
      "args": [],
      "env": {
        "SUBORDINATE_INSTANCE": "$instance",
        "SUBORDINATE_ENDPOINT_DIR": "$endpoint_dir",
        "SUBORDINATE_MCP_NO_LAUNCH": "1",
        "SUBORDINATE_LOG": "info"
      }
    }
  }
}
EOF
    start_server
    echo "agent-runbook: the editor is serving instance $instance" >&2
    echo "agent-runbook: handing the prompt to the agent" >&2
    export SUBORDINATE_MCP_CONFIG="$work/mcp.json"
    set +e
    if [ -n "$agent" ]; then
        # shellcheck disable=SC2086 # the caller's command is words on purpose.
        (cd -- "$work" && $agent <"$work/prompt.txt" >"$work/transcript.json" 2>"$work/agent.log")
    else
        (cd -- "$work" && claude -p \
            --mcp-config "$work/mcp.json" \
            --strict-mcp-config \
            --add-dir "$repo_root" \
            --allowed-tools "$allowed_tools" \
            --permission-mode bypassPermissions \
            --output-format json \
            <"$work/prompt.txt" >"$work/transcript.json" 2>"$work/agent.log")
    fi
    agent_status=$?
    set -e
    echo "agent-runbook: the session ended with status $agent_status" >&2
else
    agent_status=0
    [ -f "$work/transcript.json" ] || echo "agent-runbook: no transcript in $work" >&2
fi

# --- the verdict ------------------------------------------------------------

# The editor the session used is stopped first: the check below is the CLI's
# own, against the same plugin directory, so nothing about the verdict depends
# on the process the agent was talking to.
stop_work_keep=$keep_work
keep_work="yes"
stop_server
keep_work=$stop_work_keep

set +e
"$cli" plugin test "$plugin_id" --fixture "$fixture" --dir "$plugin_dir" --compact \
    >"$work/verify.json" 2>"$work/verify.log"
verify_status=$?
set -e

VERIFY="$work/verify.json" TRANSCRIPT="$work/transcript.json" WORK="$work" \
AGENT_STATUS="$agent_status" VERIFY_STATUS="$verify_status" python3 - <<'PY'
import json, os, sys

work = os.environ["WORK"]
verdict = {
    "work": work,
    "prompt": f"{work}/prompt.txt",
    "transcript": os.environ["TRANSCRIPT"],
    "report": os.environ["VERIFY"],
    "agent_exit": int(os.environ["AGENT_STATUS"]),
    "plugin_test_exit": int(os.environ["VERIFY_STATUS"]),
}
try:
    with open(os.environ["VERIFY"], encoding="utf-8") as handle:
        report = json.load(handle)
except (OSError, ValueError) as error:
    report = None
    verdict["reason"] = f"no plugin test report: {error}"

checks = {}
if report:
    for check in report.get("checks", []):
        checks[check.get("name", "?")] = check
    verdict["passed"] = report.get("passed")
    verdict["failed"] = report.get("failed")
    verdict["skipped"] = report.get("skipped")
    verdict["summary"] = report.get("summary")

state = checks.get("project_state", {})
changed = state.get("detail", {}).get("changed")
undoable = checks.get("undoable", {}).get("status")
verdict["changed"] = bool(changed)
verdict["undoable"] = undoable

# What the plugin said it did. The harness proves the project changed and that
# one undo reversed it; how many gaps closed is the plugin's own account, so it
# is reported beside the verdict rather than being part of it.
answer = checks.get("answers_json", {}).get("detail", {}).get("answer")
if isinstance(answer, dict):
    verdict["answer"] = {
        key: answer[key] for key in ("gaps_closed", "clips_moved", "removed") if key in answer
    }
ok = bool(report and report.get("ok")) and bool(changed) and undoable == "pass"
if not ok and "reason" not in verdict:
    verdict["reason"] = (
        "the plugin did not pass: ok="
        f"{report.get('ok') if report else None}, changed={changed}, undoable={undoable}"
    )
verdict["ok"] = ok
json.dump(verdict, sys.stdout, indent=2, sort_keys=True)
print()
sys.exit(0 if ok else 1)
PY
