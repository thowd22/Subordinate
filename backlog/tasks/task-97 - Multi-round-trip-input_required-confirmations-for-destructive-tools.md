---
id: TASK-97
title: Multi round-trip input_required confirmations for destructive tools
status: In Progress
assignee:
  - '@opus-task-97'
created_date: '2026-09-08 21:05'
updated_date: '2026-09-12 15:32'
labels:
  - mcp
milestone: m-6
dependencies:
  - TASK-94
references:
  - docs/PLAN.md
priority: medium
ordinal: 118000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Deleting sequences or overwriting files should ask before proceeding (§7).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Destructive tools (delete_sequence, remove_media, export overwrite) return input_required unless a confirm flag is set
- [ ] #2 Claude Code's elicitation dialog is verified manually
- [x] #3 Non-interactive clients can pass confirm=true
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add subordinate-mcp::confirm: the destructive tool set (sequence.delete, media.remove, export.render when its output already exists), the 'confirm' argument, and a pending-confirmation store keyed by an opaque requestState.
2. Augment the generated tool schemas so every destructive tool declares a boolean 'confirm' property; strip it before forwarding to the Command API (params are additionalProperties:false).
3. Gate Bridge::call: unconfirmed destructive calls answer with an MRTR InputRequiredResult carrying an elicitation/create request naming exactly what will be destroyed; the retry, with inputResponses and the echoed requestState, proceeds on accept and fails with mcp.confirmation_declined otherwise.
4. confirm=true short-circuits the round trip for non-interactive clients; a peer that negotiated a protocol older than 2026-07-28 gets a mcp.confirmation_required tool error telling it to pass confirm=true instead.
5. Tests: unit tests for the gate and schema augmentation, integration tests over a real engine for ask/accept/decline/confirm-true and the export overwrite case; document the flow in docs/mcp-guide.md.
6. Verify with cargo fmt, clippy pedantic and the crate's tests. AC #2 (Claude Code elicitation dialog) needs an interactive client and stays unchecked.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in bins/subordinate-mcp:

- New module src/confirm.rs: the destructive set is sequence.delete, media.remove and export.render, the last only when its output path already exists (writing a fresh file destroys nothing). A call that is not confirmed answers the 2026-07-28 multi round-trip result InputRequiredResult carrying one elicitation/create request naming the sequence, the media item or the file, plus an opaque requestState. The retry, with inputResponses and the state echoed back, runs the call on an accepted answer and fails it with mcp.confirmation_declined on a decline, a cancel, a missing answer or an unreadable one. Only an explicit yes destroys anything.
- The requestState is a handle, not a message: the pending tool and its arguments stay server-side in a bounded store (16 entries), and a state echoed back against a different call is asked about again rather than taken as consent.
- src/tools.rs adds a boolean 'confirm' property to each destructive tool's generated input schema; the gate strips it before forwarding, because the Command API methods deny unknown parameters. confirm=true runs the call in one round trip, which is what a non-interactive client uses.
- A peer that negotiated a protocol older than 2026-07-28 cannot carry input_required (the SDK refuses to send one), so bridge::downgrade turns the question into a tool error with the new code mcp.confirmation_required telling the caller to set confirm, and forgets the pending question.
- New codes mcp.confirmation_declined and mcp.confirmation_required in the crate's codes module; docs/mcp-guide.md gains a 'Confirming a destructive call' section and both codes in its failure table.
- Bridge::call now returns CallToolResponse rather than CallToolResult; the existing integration tests were updated to unwrap the completed variant.

Verification: cargo fmt --all --check clean; cargo clippy -p subordinate-mcp --all-targets -- -D warnings clean; cargo test -p subordinate-mcp passes (46 unit tests including 11 new ones in confirm:: and bridge::, plus the new tests/confirmations.rs which drives ask/accept/decline/confirm-true and the export overwrite case against a real engine over the real socket transport).

AC #2 is not checked: verifying Claude Code's elicitation dialog needs an interactive MCP client and a running editor, neither of which this environment has. The server side of that dialog is covered by tests/confirmations.rs, which asserts the wire shape of the elicitation/create request the client would render.

cargo clippy --workspace --all-targets -- -D warnings also passes (6m12s, exit 0) with the machine's GStreamer environment exported.

2026-09-12 supervisor handoff: criterion 2 (Claude Code's elicitation dialog) is a manual check: register subordinate-mcp in a Claude Code session (docs/mcp-guide.md .mcp.json snippet), call a destructive tool without confirm, and observe the dialog.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The three destructive MCP tools now ask before they act. sequence_delete, media_remove and an export_render that would overwrite an existing file answer the 2026-07-28 input_required result with an elicitation naming exactly what is about to go, and run only when the same call is retried with an accepted answer and the echoed requestState; a decline, a cancel or a missing answer fails with mcp.confirmation_declined and touches nothing. A client with nobody to ask sets the tools' new confirm argument and is served in one round trip, and a peer on an older protocol is told to do so with mcp.confirmation_required. Verified with cargo fmt --all --check, cargo clippy -p subordinate-mcp --all-targets -- -D warnings, and cargo test -p subordinate-mcp, whose new tests/confirmations.rs drives ask, accept, decline and confirm=true against a real engine over the socket transport. AC #2 is left unchecked: Claude Code's dialog needs an interactive client this environment does not have.
<!-- SECTION:FINAL_SUMMARY:END -->
