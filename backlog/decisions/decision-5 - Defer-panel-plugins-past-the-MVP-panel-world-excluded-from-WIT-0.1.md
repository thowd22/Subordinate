---
id: decision-5
title: Defer panel plugins past the MVP; panel world excluded from WIT 0.1
date: '2026-09-08 20:52'
status: accepted
---
## Context

Panel plugins need a UI description mechanism (declarative widgets or webview HTML) that is not settled and not required for the MVP extension points.

## Decision

Defer panel plugins past the MVP. The panel world is excluded from subordinate:plugin@0.1.0.

## Consequences

WIT 0.1 ships with effect, audio-effect, importer, exporter, analyzer, command and mcp-tools worlds only. Revisit declarative JSON widgets versus MCP-Apps-style HTML after the MVP.
