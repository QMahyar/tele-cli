# 08 — Session lock files cleaned up on release

**What to build:** Every client connect creates `sessions/<name>.session.lock` (0 bytes) that stays on disk forever — hundreds of runs leave dozens of stale files. The lock must be removed when the owning process releases it.

- When a client disconnects and no other process holds the lock, the `.session.lock` file should be deleted (best-effort — a failed delete must never error the command).
- Race safety: if another process currently holds the lock, the delete fails and the file stays — that's correct. On Windows, deleting a file another process has open fails naturally, so best-effort deletion after dropping our own handle is safe. Design the Drop order so our handle is closed BEFORE the delete attempt (e.g. hold the lock as `Option<File>` in `ClientGuard` and `take()` it in `Drop`, or implement Drop for `LockedSession`).
- `remove_session` already deletes both files — keep that. The fix targets normal connect/disconnect cycles.
- Behavior of mutual exclusion (second concurrent client fails to open) must be unchanged — the existing tests `open_session_lock_is_released_on_drop`, `open_session_lock_is_held_while_held` must keep passing.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

**Verified context (commit a647834):** `src/session.rs` — `lock_path` (30-31), `open_session` (84-106, `try_lock` at 95), `remove_session` deletes both files (68-69); `src/client.rs` — `ClientGuard._session_lock: std::fs::File` (17), connect (33, 62), existing `Drop` impl (67-71) disconnects the client. Rust's `File::try_lock` (stable, 1.89+) is in use.

**Acceptance criteria:**
- [ ] RED test first: after `open_session` + drop, the lock file no longer exists (unit test using TEST_ENV_LOCK + temp session dir)
- [ ] Held lock (second open while first held) still fails; lock file survives while held
- [ ] Normal commands leave no `.session.lock` files behind after exit
- [ ] No comments added; AGENTS.md conventions followed
- [ ] `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check` pass
- [ ] Commit as `fix:` prefix, one logical change, on branch `fix/08-session-locks`