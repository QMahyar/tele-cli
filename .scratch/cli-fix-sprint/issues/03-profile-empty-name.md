# 03 — profile set rejects empty name

**What to build:** `tele profile set --name ""` currently passes validation, hits Telegram's RPC, and fails with `FIRSTNAME_INVALID` (exit 3, ALL_FAILED); with `--dry-run` it falsely reports success (exit 0). Telegram does not allow an empty first name, so the CLI must reject it before connecting.

- `--name ""` or whitespace-only `--name "   "` → Usage error (exit 1), before connect, both live and `--dry-run`.
- The split into first/last name must not change for valid input ("John Doe" → first "John", last "Doe" as today).
- `--bio ""` must remain ALLOWED (empty bio is a legitimate "clear bio" operation — do not reject it).
- `--photo` behavior unchanged.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

**Verified context (commit a647834):** `src/commands/profile.rs` `validate_set` (127-137) checks `is_none()` but not emptiness; name split in `set` (192-201); existing tests in `tests` module (261+).

**Acceptance criteria:**
- [ ] RED test first: `validate_set` with empty/whitespace name → Usage error; valid name still ok; empty bio still ok
- [ ] CLI behavior: `profile set --name ""` exits 1 with UsageError envelope in --json mode; `--dry-run` also exits 1
- [ ] `profile set --bio ""` still accepted
- [ ] No comments added; AGENTS.md conventions followed
- [ ] `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check` pass
- [ ] Commit as `fix:` prefix, one logical change, on branch `fix/03-profile-empty-name`