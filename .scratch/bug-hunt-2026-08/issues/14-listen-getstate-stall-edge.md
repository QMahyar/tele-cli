# 14 — listen surfaces initial GetState failure instead of stalling silently

**What to build:** If the initial `GetState` fails (revoked session, network failure at startup) AND the session has no saved update state, `tele listen` must fail or reconnect loudly — never block forever emitting nothing with no error, no reconnect, and no timeout message. The swallowed GetState error must be surfaced.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: stream startup with a failing GetState produces a visible error path (error or bounded retry), not an infinite silent wait
- [ ] Offline test: normal startup unaffected (catch_up behavior intact)
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 2.2 residual edge (grammers swallows GetState error; pristine message box → Gap error swallowed → recv blocks forever)