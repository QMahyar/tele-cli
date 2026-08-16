# 01 — msg delete reports no-op/partial deletion as success; add self-only option

**What to build:** `tele msg delete` must tell the truth about what was deleted. When the server deletes fewer messages than requested (others' messages in private chats, already-deleted ids, no permission), the command must report `requested` vs `deleted` counts and exit 2 (partial) instead of exit 0 with `{"deleted":0}`. Deleting must not be hardcoded to both-sides revocation: a self-only path must exist (raw `messages.deleteMessages { revoke: false }` for the user branch), so users can delete only for themselves in private chats.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: delete report distinguishes `requested`/`deleted`; partial deletion → exit 2, full no-op → non-zero exit, full success → 0
- [ ] Offline test: `--revoke`/self-only flag plumbing serializes `revoke: false` (raw path), default stays `revoke: true`
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 8.H1 (msg.rs delete_report, grammers hardcodes revoke:true)