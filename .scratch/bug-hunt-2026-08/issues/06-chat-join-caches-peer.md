# 06 — chat join caches the joined chat's access hash

**What to build:** After `tele chat join` succeeds, the joined chat's peer must be cached in the session exactly like `tele chat create` does — so immediate follow-up commands (`participants --chat <id>`, `leave --chat <id>`, `send`) resolve the id without `PEER_NOT_CACHED`/Dropped failures.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: the join handler caches the returned peer (accept_invite_link/join_chat result) via the same cache path as create
- [ ] Offline test: join via username and via invite link both cache
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 10.4 (join discards `Option<Peer>`)