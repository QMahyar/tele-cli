# 07 — empty TELE_API_HASH rejected up front

**What to build:** `TELE_API_HASH=` (set-but-empty in `.env` or environment) must fail credential loading with a clear error like `api_id` gets, instead of flowing an empty hash into login/connect and failing later with cryptic RPC errors. Whitespace-only hashes rejected too.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: empty and whitespace-only `TELE_API_HASH` are rejected at credential load (exit 1, clear message)
- [ ] Offline test: `.env` empty-value parse path covered (env-var path already filters empties)
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 3.5 (only presence is checked)