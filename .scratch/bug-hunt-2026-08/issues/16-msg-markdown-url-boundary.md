# 16 — validate_markdown only rejects real tg://user mentions

**What to build:** `tele msg send --format markdown` must accept normal URLs whose destination merely contains `tg://user?id=` (e.g. `[x](https://example.com/tg://user?id=abc)`). Only text grammers would actually parse as a `tg://user?id=` mention must be rejected.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: URL link containing the substring is accepted
- [ ] Offline test: genuine tg://user mentions with invalid/non-numeric id still rejected
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 8.L1 (raw substring scan, no URL-boundary check)