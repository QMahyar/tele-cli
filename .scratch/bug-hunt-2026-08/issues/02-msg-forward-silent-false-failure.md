# 02 — msg forward --silent must not fail after a succeeded forward

**What to build:** `tele msg forward --silent` must never fail an account when the `forwardMessages` RPC itself succeeded. When no new message ids can be extracted from the response (server answered with an Updates variant lacking `UpdateMessageId`), the command must succeed with a warning and empty/partial `forwarded` ids rather than erroring — retrying must not be able to duplicate messages. The `chunk[i]` indexing must be guarded so a partial id extraction cannot panic.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: response without matching `UpdateMessageId` → ok outcome with warning, not an error
- [ ] Offline test: partial id extraction cannot index out of bounds (panic-guard)
- [ ] Offline test: `--silent` and non-silent paths agree on counts semantics
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 8.H2 (forwarded_ids error branch + chunk indexing)