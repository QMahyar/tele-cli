# 08 — write_config preserves comments and unknown keys

**What to build:** `tele account add`/`remove` must not destroy user comments, formatting, or unknown tables in `config.toml`. The write path must mutate the existing TOML document (e.g. via `toml_edit`) instead of re-serializing the parsed struct, so a hand-edited config survives account management intact.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: config with comments + unknown table survives `account add`/`remove` round-trip byte-for-byte apart from the intended key change
- [ ] Offline test: atomic tmp+rename write behavior retained
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 3.7 (toml::to_string_pretty full rewrite)