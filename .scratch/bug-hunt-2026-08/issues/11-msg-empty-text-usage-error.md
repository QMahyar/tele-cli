# 11 — empty/whitespace --text rejected as usage error

**What to build:** `tele msg send --text ""` (or whitespace-only) must fail up front with a usage error (exit 1) instead of connecting and getting server `MESSAGE_EMPTY` (exit 3). `msg edit` gets the equivalent validation, context-aware: clearing a media caption with an empty string stays legal, but empty text with no media context is a usage error.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: send with empty/whitespace text → usage error before connect
- [ ] Offline test: edit with empty text + media context allowed; edit with empty text + no media → usage error
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 8.M1 (validate_send only rejects None)