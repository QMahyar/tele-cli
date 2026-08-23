# F2: Telethon .session import read-half (libsql approved)

**Branch:** `feat/f2-telethon-import` · **Files:** `src/session.rs`, `src/commands/account.rs`, `Cargo.toml` · **Deps:** F-none; account.rs free

## Goal

Complete kernel.session-port: read foreign Telethon SQLite sessions and author native ones (authoring half already shipped behind `write_native_from_telethon` seam).

## Acceptance

- [ ] Add direct dep matching grammers-session's OWN engine (check Cargo.lock: libsql version already transitive — pin the SAME minor; zero new compile cost; do NOT add rusqlite)
- [ ] Implement `parse_telethon_session(file) -> TelethonSessionData{auth_key:[u8;256], dc_id, user_id?, ...}` reading Telethon's schema (`version` table, `sessions` table legacy cols + newer split layout if trivially detectable; support at minimum the classic single-table schema; honest error listing unsupported layouts)
- [ ] Wire into existing `import-session --from-telethon` seam replacing the NotImplemented error; full flow offline-testable via fixture .session files built with libsql in tests (magic-bytes rejection stays)
- [ ] Security unchanged: keys never printed; perms restricted; garbage rejection tests still green
- [ ] Gates green; live import test remains a manager/user checklist item

## Boundaries

Only `src/session.rs`, `src/commands/account.rs`, plus the one Cargo.toml dep line.
