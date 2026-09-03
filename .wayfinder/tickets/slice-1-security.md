# Ticket: Slice 1 — Security fixes

Type: `wayfinder:task` (AFK execution)
Branch: `slice/1-security`
Blocks: Slice 2
Question: Fix the 2 HIGH + 2 MEDIUM + LOW security findings from the security review agent.

Scope (ranked):
1. HIGH — `raw` op bypasses confirm gate in serve/MCP (raw.rs:187-204, serve.rs:804).
2. HIGH — Unix `export-session` doesn't tighten mode on existing dest (session.rs:302, fs_util.rs:114).
3. MEDIUM — `create_dir_private` predictable temp name symlink race + `/tmp` fallback (fs_util.rs:24-45, config.rs:45).
4. MEDIUM — Windows truncate-open fallback follows symlinks (fs_util.rs:132-141).
5. LOW — export can clobber another account's live session (session.rs:297-303).
6. LOW — Telethon import logs full path (session.rs:544-547).
7. LOW — stale-temp sweep kills other processes' downloads (download.rs:247-262).
8. LOW — `.session-wal`/`-shm` not in sensitive-suffix list (validate.rs:79-96).

Done = clippy + `cargo test` green, fixes landed, capabilities/security docs updated where behavior changed.
