# W2-2: kernel.session-port — export/import + Telethon converter

**Branch:** `feat/w2-2-session-port` · **Files:** `src/session.rs`, `src/commands/account.rs` · **Deps:** none (post-W1 merges)

## Goal

Portable session backups between machines + one-way Telethon `.session` migration into tele.

## Acceptance

- [ ] `tele account export-session --name A [--out PATH]`: copies `{app}/sessions/A.session` (+ lockfile excluded) to destination; output file gets restricted owner-only perms via fs_util patterns; stdout JSON carries sha256 + byte size; refuses exporting while source is locked by a live process (try_lock probe) with honest error
- [ ] `tele account import-session --file P [--as NAME]`: validates target naming rules (validate_name), refuses clobbering an existing account without `--force`, restricts imported file perms, prints resulting account name
- [ ] `tele account import-session --file P --from-telethon [--as NAME]`: reads Telethon SQLite schema (version table + sessions table auth_key/dc_id/user_id/etc.) and writes an equivalent native session. DEPENDENCY HONESTY CLAUSE: inspect whether grammers-session 0.10 publicly exposes raw SQL access on its SqliteSession or accepts injected auth_key/dc state; if NOT, STOP after building everything around the conversion seam and report exactly which capability needs a `rusqlite` direct-dependency approval — do NOT add dependencies yourself
- [ ] Security: never print auth keys/hashes; exported file path warnings on stderr in human mode; tests use fixture files created via the same crate APIs (no network)
- [ ] Offline tests: roundtrip export/import identity, locked-source refusal, force semantics, Telethon fixture parse (if seam allows); gates green

## Boundaries

Only `src/session.rs` + `src/commands/account.rs`.
