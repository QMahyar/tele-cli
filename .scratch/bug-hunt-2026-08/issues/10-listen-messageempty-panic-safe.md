# 10 — listen must not panic on MessageEmpty edited-channel messages

**What to build:** `tele listen` must survive an `EditChannelMessage` update whose raw message is `messages.messageEmpty` with no peer — currently the account stream dies with a panic (exit 3). The event must be skipped (or emitted with null fields), never panicking, and the stream must keep running.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: serialization path for a peer-less empty message returns a row (or skips) without panicking
- [ ] Offline test: message edited event with empty message produces a non-panic outcome; stream continues (per-account task not killed)
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 12.3 (grammers `peer_id()` expect; reachable via message serialization)