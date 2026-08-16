# 17 — chat participants on basic groups cannot panic

**What to build:** `tele chat participants` on a basic group must never panic when Telegram omits a participant's user from the response — the CLI must pre-check membership (or iterate the returned users directly) instead of hitting grammers' internal `take_user().unwrap()`. Missing users become an error or a skipped row, never a process panic.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: participants iteration with a participant whose user is absent from the response → no panic, graceful outcome
- [ ] Offline test: normal participant listing unchanged
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 10.9 (grammers participant.rs take_user().unwrap(), reachable via basic-group GetFullChat)