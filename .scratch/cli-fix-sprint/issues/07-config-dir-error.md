# 07 — Clear error when --config points at a directory

**What to build:** `tele --config <directory> account list` fails with a confusing raw OS error (`Access is denied. (os error 5)` on Windows). The CLI must detect a directory and fail with a clear ConfigError naming the path.

- `--config` pointing at an existing directory → clean error: `failed to read config: <path> is a directory` (or similar), exit 1, `ConfigError` type in `--json` envelope. No OS-error text.
- Missing config path keeps today's behavior (defaults — do not change).
- Malformed TOML keeps today's parse error (already good).

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

**Verified context (commit a647834):** `src/config.rs` `read_config` (261-270): `if !cfg_path.exists()` → defaults; else `std::fs::read_to_string(cfg_path)?` — a directory passes `exists()` and blows up with the OS error. Error wraps into `TeleError::Config` at `load_config` (253).

**Acceptance criteria:**
- [ ] RED test first: read_config on a directory path returns a clear error (unit test in config.rs tests, using TEST_ENV_LOCK + temp dir)
- [ ] CLI: `--config <dir> account list --json` → exit 1, ConfigError envelope, message names the path and says it's a directory
- [ ] Nonexistent `--config` still silently defaults (existing test `missing_config_file_is_defaults_not_error` must keep passing)
- [ ] No comments added; AGENTS.md conventions followed
- [ ] `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check` pass
- [ ] Commit as `fix:` prefix, one logical change, on branch `fix/07-config-dir-error`