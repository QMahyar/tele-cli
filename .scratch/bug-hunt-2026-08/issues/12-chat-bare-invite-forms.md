# 12 — bare t.me/+hash invite forms accepted by chat join

**What to build:** `tele chat join --chat t.me/+<hash>` and `t.me/joinchat/<hash>` (scheme-less pastes) must join the chat. The join path must normalize `t.me/...` to `https://t.me/...` before invite-link parsing (full URLs and bare `+hash`/`hash` already work), and give non-invite t.me forms their existing targeted errors.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: `t.me/+hash` and `t.me/joinchat/hash` normalize to full URLs accepted by the invite parser
- [ ] Offline test: existing full-URL and bare-hash paths still work; unrelated t.me links still resolve as links/usernames
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 10.3 (no scheme normalization; only full https:// parses)