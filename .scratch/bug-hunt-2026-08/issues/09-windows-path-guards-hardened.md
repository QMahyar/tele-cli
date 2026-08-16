# 09 — Windows path guards hardened (upload + download)

**What to build:** The app-data/sessions path guards must not be bypassable on Windows: (a) comparison must be case-insensitive where Windows paths are case-insensitive; (b) the guard must not fall back to the raw un-canonicalized path when `canonicalize` fails — resolve the full path (create-then-verify) before checking; (c) uploads of non-existent files must be rejected as a usage error before connecting instead of exiting 3 with a raw IO error.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: differently-cased path into app data is rejected on the download-dir guard
- [ ] Offline test: guard with a not-yet-existing directory tail still resolves inside/outside app data correctly
- [ ] Offline test: upload `--file <nonexistent>` → usage error (exit 1) before connect
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md findings 9.2 (case-sensitive starts_with + raw fallback) and 8.L4 (nonexistent-file path)