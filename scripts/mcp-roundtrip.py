#!/usr/bin/env python3
"""Prove an MCP round-trip against a `subordinate-mcp` binary.

Speaks the stdio transport the way any MCP client does -- one JSON-RPC message
per line -- runs the `initialize` handshake, then calls the tools named on the
command line in order and prints each result. The default pair is
`project.new` followed by `timeline.get_state`, which is the round-trip the
desktop-runner smoke job asserts (TASK-137): a mutation goes in, and the
timeline that comes back is the one it produced.

Tool names are the Command API method names; the bridge publishes them with
the dot replaced by an underscore, and this script does that translation so
the call reads the way the API is documented.

    scripts/mcp-roundtrip.py --mcp /usr/local/bin/subordinate-mcp \
        'project.new={"name":"Desktop smoke"}' 'timeline.get_state={}'

Exit status is 0 only if every call returned a result rather than an error.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys

PROTOCOL_VERSION = "2025-06-18"


class Bridge:
    """A `subordinate-mcp` process, spoken to over its stdin and stdout."""

    def __init__(self, command: list[str], timeout: float) -> None:
        self.process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=None,
            text=True,
            bufsize=1,
            env=os.environ.copy(),
        )
        self.timeout = timeout
        self.next_id = 1

    def send(self, message: dict) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def receive(self) -> dict:
        assert self.process.stdout is not None
        line = self.process.stdout.readline()
        if not line:
            raise SystemExit("subordinate-mcp closed its output")
        return json.loads(line)

    def request(self, method: str, params: dict) -> dict:
        request_id = self.next_id
        self.next_id += 1
        self.send(
            {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        )
        while True:
            message = self.receive()
            # Notifications can arrive at any time; only the matching reply ends
            # the wait.
            if message.get("id") == request_id:
                return message

    def close(self) -> None:
        # Closing stdin is how a stdio MCP session ends, and it is also how the
        # bridge is told to stop the headless engine it launched.
        if self.process.stdin is not None:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=self.timeout)
        except subprocess.TimeoutExpired:
            self.process.kill()


def parse_call(argument: str) -> tuple[str, dict]:
    """Split `method={json}` into the method name and its arguments."""
    method, _, raw = argument.partition("=")
    arguments = json.loads(raw) if raw.strip() else {}
    if not isinstance(arguments, dict):
        raise SystemExit(f"{method}: arguments must be a JSON object")
    return method, arguments


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--mcp",
        default="subordinate-mcp",
        help="the subordinate-mcp binary (default: the one on PATH)",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=60.0,
        help="how long to wait for the bridge to exit at the end",
    )
    parser.add_argument(
        "calls",
        nargs="*",
        default=None,
        help='calls as method={json}, e.g. \'project.new={"name":"Smoke"}\'',
    )
    options = parser.parse_args()
    calls = [
        parse_call(call)
        for call in (options.calls or ['project.new={"name":"MCP round-trip"}',
                                       "timeline.get_state={}"])
    ]

    bridge = Bridge([options.mcp], options.timeout)
    failures = 0
    try:
        initialized = bridge.request(
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "mcp-roundtrip", "version": "1"},
            },
        )
        if "error" in initialized:
            print(f"initialize failed: {initialized['error']}", file=sys.stderr)
            return 1
        server = initialized["result"]["serverInfo"]
        print(f"connected to {server['name']} {server.get('version', '')}".strip())
        bridge.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

        for method, arguments in calls:
            reply = bridge.request(
                "tools/call",
                {"name": method.replace(".", "_"), "arguments": arguments},
            )
            if "error" in reply:
                print(f"{method}: error {reply['error']}", file=sys.stderr)
                failures += 1
                continue
            result = reply["result"]
            if result.get("isError"):
                print(f"{method}: tool error {json.dumps(result)}", file=sys.stderr)
                failures += 1
                continue
            payload = result.get("structuredContent")
            if payload is None:
                payload = [
                    block.get("text") for block in result.get("content", [])
                ]
            print(f"{method}: {json.dumps(payload)[:2000]}")
    finally:
        bridge.close()

    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
