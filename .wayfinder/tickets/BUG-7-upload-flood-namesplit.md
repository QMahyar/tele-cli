# BUG-7: Upload FLOOD_WAIT loses JSON keys + name-split wipes last name

**Source:** deep-dive plumbing review Task B gap + dialog-group M6.

## Question/Problem

1. `src/commands/msg.rs:483-486`: upload-time FLOOD_WAIT maps to `TeleError::Invocation(msg, secs)` — loses `code`/`name` additive JSON keys that send-path Rpc errors carry. Inconsistent machine contract.
2. `src/commands/profile.rs:197-206`: `profile set --name "John"` sends `last_name: Some("")` → silently clears existing surname; internal double-space yields leading-space last name; no length validation (first/last 64, bio ~140/70).

## Acceptance criteria

- [ ] Upload FLOOD_WAIT constructs `TeleError::Rpc` carrying code/name/value so JSON keys match the send path (test asserts key parity).
- [ ] Name split: send `last_name` ONLY when a space-separated tail exists; trim segments; reject >64-char first/last and >140 bio client-side as Usage errors before connect.
- [ ] Unit tests for split logic (property or table) incl. "John", "John Smith", "A  B", 65-char inputs.

## Verification

- [ ] clippy -D warnings / fmt check / full `cargo test`

## Files

- `src/commands/msg.rs`, `src/commands/profile.rs`, tests

## Constraints

Branch `fix/bug-7-upload-flood-namesplit`. Only these regions of msg.rs (upload error mapping ~line 483) — another ticket owns the get-loop region. No comments.
