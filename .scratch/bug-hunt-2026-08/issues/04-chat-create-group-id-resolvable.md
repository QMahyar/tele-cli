# 04 — chat create --kind group id must be resolvable via --chat

**What to build:** The id `tele chat create --kind group` prints must round-trip: immediately after creating a basic group, `tele chat participants --chat <printed id>` (or any follow-up command) must resolve it. Today the printed positive chat id cannot be resolved because peer resolution probes user/channel kinds but never the chat kind for positive ids.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: positive-id resolution probes the chat kind (`PeerId::chat`) when user/channel probes miss
- [ ] Offline test: a cached basic group by positive id resolves to the correct peer kind (no `CHANNEL_INVALID`/Dropped path)
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 10.2 (cached_ref probes user+channel only)