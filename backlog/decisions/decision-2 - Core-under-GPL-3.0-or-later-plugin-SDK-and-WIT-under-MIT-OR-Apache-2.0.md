---
id: decision-2
title: 'Core under GPL-3.0-or-later, plugin SDK and WIT under MIT OR Apache-2.0'
date: '2026-09-08 20:52'
status: accepted
---
## Context

The core should discourage closed forks while the plugin ecosystem should be open to any licence, including closed-source commercial plugins.

## Decision

Core crates and binaries: GPL-3.0-or-later. Plugin SDK crates and wit/ interface definitions: MIT OR Apache-2.0.

## Consequences

Plugins link only against SDK and WIT, so they may use any licence. Contributors to the core accept GPL. Licence files are part of the workspace scaffold task.
