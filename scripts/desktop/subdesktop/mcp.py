"""A minimal MCP stdio client, for flows that drive `subordinate-mcp`.

`scripts/mcp-roundtrip.py` is the command-line version of this and stays the
thing a person runs by hand; this is the same protocol as a library, because a
flow makes a dozen calls and has to read each result rather than print it.

Tool names are the Command API method names with the dot replaced by an
underscore (docs/mcp-guide.md), and this does that translation, so a flow reads
the way the API is documented: `call("timeline.add_clip", {...})`.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

PROTOCOL_VERSION = "2025-06-18"

#: How the bridge's `initialize` instructions open the sentence naming what the
#: session is driving (bins/subordinate-mcp/src/bridge.rs).
EDITOR_NOTE = "Connected to the editor already running at"


def project_from_result(state):
    """Read a project through Command API and project-file envelopes."""
    while isinstance(state, dict):
        if isinstance(state.get("sequences"), list):
            return state
        if "project" not in state:
            break
        state = state["project"]
    raise AssertionError(f"project.get did not contain a project: {json.dumps(state)[:600]}")


class McpError(RuntimeError):
    """A tool call the editor rejected, or a bridge that would not start."""


class Bridge:
    """A `subordinate-mcp` process, spoken to over its stdin and stdout."""

    def __init__(
        self,
        binary: str | Path,
        *,
        cwd: Path | str | None = None,
        env: dict[str, str] | None = None,
        require_editor: bool = True,
        log: Path | str | None = None,
    ) -> None:
        environment = dict(os.environ)
        environment.update(env or {})
        self._log_handle = None
        if log is not None:
            Path(log).parent.mkdir(parents=True, exist_ok=True)
            self._log_handle = Path(log).open("wb")
        self.process = subprocess.Popen(
            [str(binary)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self._log_handle or None,
            text=True,
            bufsize=1,
            cwd=str(cwd) if cwd else None,
            env=environment,
        )
        self._next_id = 1
        self.connection = ""
        self._handshake(require_editor)

    # ------------------------------------------------------------------ frames

    def _send(self, message: dict) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def _request(self, method: str, params: dict) -> dict:
        request_id = self._next_id
        self._next_id += 1
        self._send(
            {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        )
        assert self.process.stdout is not None
        while True:
            line = self.process.stdout.readline()
            if not line:
                raise McpError("subordinate-mcp closed its output")
            message = json.loads(line)
            if message.get("id") == request_id:
                return message

    def _handshake(self, require_editor: bool) -> None:
        reply = self._request(
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "desktop-flow", "version": "1"},
            },
        )
        if "error" in reply:
            raise McpError(f"initialize failed: {reply['error']}")
        instructions = reply["result"].get("instructions", "")
        at = instructions.find(EDITOR_NOTE)
        self.connection = (
            " ".join(instructions[at:].split()) if at != -1 else "(headless)"
        )
        if require_editor and at == -1:
            raise McpError(
                "the bridge is not driving a running editor: " + instructions[-300:]
            )
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    # ------------------------------------------------------------------- calls

    def call(self, method: str, arguments: dict | None = None):
        """Call one Command API method and return its structured result."""
        reply = self._request(
            "tools/call",
            {"name": method.replace(".", "_"), "arguments": arguments or {}},
        )
        if "error" in reply:
            raise McpError(f"{method}: {json.dumps(reply['error'])}")
        result = reply["result"]
        if result.get("isError"):
            raise McpError(f"{method}: {json.dumps(result)[:2000]}")
        payload = result.get("structuredContent")
        if payload is None:
            texts = [block.get("text") for block in result.get("content", [])]
            payload = texts[0] if len(texts) == 1 else texts
            if isinstance(payload, str):
                try:
                    payload = json.loads(payload)
                except json.JSONDecodeError:
                    pass
        return payload

    def close(self) -> None:
        """End the session; closing stdin is how a stdio MCP client leaves."""
        if self.process.stdin is not None:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self.process.kill()
        if self._log_handle is not None:
            self._log_handle.close()

    def __enter__(self) -> "Bridge":
        return self

    def __exit__(self, *_exc) -> None:
        self.close()
