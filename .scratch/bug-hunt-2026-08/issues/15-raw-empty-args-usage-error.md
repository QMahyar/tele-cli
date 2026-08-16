# 15 — raw --args empty chat/channel is a usage error

**What to build:** `tele raw <method> --args '{"chat":""}'` (or `"channel":""`) must fail before connecting with a usage error (exit 1), like sibling commands treat empty targets — not a fabricated `INVALID_PEER_ID` RPC error (exit 3).

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: empty-string required peer fields → usage error (exit 1), no connect
- [ ] Offline test: valid args unchanged; missing-key errors unchanged
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 13.4 (req_str accepts "")