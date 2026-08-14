# Implementation Plan: Tele-Cli (Rust)

## Overview

Build the session kernel first, then ship vertical CLI slices that each mark a `docs/capabilities.md` row `done`. Live Telegram runs use sessions you provide (manual, not CI). MCP and the agent skill stay last.

## Architecture Decisions

- **`src/client.rs` owns grammers.** CLI commands resolve accounts, call kernel, print. No `Client` construction outside `src/client.rs`.
- **Connect per command** until listen. One client per account per process; RAII disconnect.
- **Selection:** `--account` (repeatable, or `all`) ∪ `--tag`. Empty selection is an error except `tele account list`.
- **Friendly methods first.** Raw `tl::functions.*` only in the `tele raw` registry or when the matrix says no friendly path.
- **Flood:** `ClientConfiguration.flood_sleep_threshold = 60` + `AutoSleep` retry. Executor sequential unless `--parallel N` (`1..3`). Per-account results always returned (success / flood / error).
- **Sessions:** `{app_dir}/sessions/{name}.session` (grammers `FileSession`). Never CWD. Never two clients on one file.
- **`tele raw` is a typed registry** (static Rust TL): match arm per supported method; clear error otherwise.

## Dependency graph

```
Cargo skeleton
    │
    ├── config + accounts + session paths
    │       │
    │       ├── client (grammers factory: proxy, flood, reconnect)
    │       │       │
    │       │       ├── executor (fan-out)
    │       │       │
    │       │       ├── account login/logout/list   ← first Telegram slice
    │       │       ├── msg send (+ schedule)
    │       │       ├── chat join/leave
    │       │       ├── dialog list
    │       │       ├── listen
    │       │       └── raw registry
    │       │
    │       └── serialize + --json / --dry-run
    │
    └── capabilities contract test
```

## Task List

### Phase 0: Scaffold

- [x] Task 1: Cargo skeleton (clap, modules, cargo test green)
- [x] Task 2: Capability contract test (matrix ↔ declared CLI groups)

### Phase 1: Kernel

- [x] Task 3: Config + `.env` (`TELE_API_ID` / `TELE_API_HASH`) + TOML accounts/tags/proxy
- [x] Task 4: Session path policy + account selection (`--account` / `--tag`; default = all sessions)
- [x] Task 5: Client factory + executor (sequential / parallel ≤3 / flood surfacing)
- [x] Task 6: Output (`--json`, tables, `--dry-run`, `--quiet`, `TELE_LOG`)

### Phase 2: First Telegram slice (Auth)

- [x] Task 7: `tele account add|list|status`
- [x] Task 8: `tele account login` (code + 2FA + QR — QR via raw `auth.exportLoginToken`)
- [x] Task 9: `tele account logout` vs `remove` (local-only)

### Phase 3: Daily operators

- [x] Task 10: `tele msg send` (+ `--schedule`, `--file`)
- [x] Task 11: `tele msg edit|delete|forward|pin|get|read|react|search` (+ `download`)
- [x] Task 12: `tele chat join|leave|invite|participants|kick|admin|admin-log|stats|create`
- [x] Task 13: `tele dialog list|drafts|archive|delete` + `tele topic create|list`

### Phase 4: Listen + raw

- [x] Task 14: `tele listen` (default NewMessage; `--events` allowlist; JSONL)
- [x] Task 15: `tele raw` (typed registry)

### Phase 5: Remaining `want` rows

- [x] contact: list/add/block/unblock
- [x] profile: get/set (name, bio, photo)
- [x] privacy: get/set rules
- [x] takeout: start/export/finish
- [x] kernel.proxy — wired (global + per-account, socks5-only; see `tasks/todo.md`)
- [x] kernel.peers — phone targets via raw `contacts.ImportContacts`
- [ ] msg.poll — deferred to `later`: grammers 0.10 has no friendly poll (no raw arm yet)

### Phase 6: Last (do not start, ask first)

- [ ] MCP `tele mcp serve`
- [ ] Agent skill

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Session file locked / AUTH_KEY_DUPLICATED | High | One client per session; refuse if lock held |
| Flood / SpamBot on fan-out | High | Sequential default; cap 3; surface wait; live tests only hit `me` |
| grammers layer lag vs Telegram | Med | Matrix + `tele raw` registry; manual bump process |
| QR login lacks friendly helper | Med | Raw `auth.exportLoginToken` + update stream (documented pattern) |
| Login interactivity vs agents | Med | Prompts for humans; flags for non-TTY |
| Scope explosion (“full from start”) | High | Matrix is DoD; slices mark rows; unpublished until `want` is `done` |

## Audit

Comprehensive audit completed 2026-08-14: `docs/audit-2026-08.md` — 96 findings across security, bugs, performance, test coverage, and UX. Implementation tracked below.

## Open Questions

None blocking. Live verification checklist and status: `tasks/todo.md` (verified
2026-08-13 against real sessions; remaining user-side items listed there).
