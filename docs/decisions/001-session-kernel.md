# ADR-001: Session kernel as the product core

## Status
Accepted

## Date
2026-08-13

## Context
Multiple real phone accounts must persist login and never share Telethon SQLite sessions. A previous Go client was discarded. Telethon documents that two clients on one session lock the DB or trigger `AUTH_KEY_DUPLICATED`.

## Decision
Own a kernel (`core/`) that: resolves named accounts + tags, maps each name to `{app_dir}/sessions/{name}.session`, builds one `TelegramClient` at a time per file, and fans out work sequentially by default.

## Alternatives considered

### CWD `Name.session`
Rejected: working-directory drift relogins; easy to commit or collide.

### StringSession in config
Rejected for default: a string is easier to leak into logs/chat than a 0600 file. May revisit as export-only.

### Always-on daemon from day one
Deferred. Listen/MCP may force it (ADR later). Connect-per-command is simpler and avoids a permanent lock.

## Consequences
- CLI must not construct clients itself.
- Logout vs local file delete must stay distinct (`log_out` revokes).
- App data dir differs per OS (`%APPDATA%/telecli` vs XDG).
