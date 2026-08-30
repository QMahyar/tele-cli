# ADR 007: Product scope v1 (Rust era)

- Status: accepted
- Date: 2026-08-13
- Supersedes: spec §2 (scope), spec §4 (non-functional requirements)
- Context after: ADR-006 (Rust/grammers pivot)

## Context

The original Python-era spec (`docs/ideas/tele-cli.md`, now deleted) carried a
v1 out-of-scope list and a set of non-functional requirements. ADR-006 replaced
the tech stack but did not formally capture those scope boundaries. This ADR
preserves them for the Rust era.

## Decision

### In scope (v1)

- Full grammers user-client surface (friendly methods + `tele raw` registry)
- Named accounts with persistent sessions (one `FileSession` per account, outside
  CWD, never shared across processes)
- Sequential default; `--parallel N` (N clamped 1–32) with FloodWait /
  SlowModeWait honour
- Human CLI (`tele` binary) + `--json` / JSONL for agents
- Stay-connected listen mode (`tele listen`, filtered events)
- Raw TL access (`tele raw <method> ...`)
- Living capability matrix in-repo (`docs/capabilities.md`)

### Out of scope (v1)

- Bot-token-first product (phone accounts only)
- Custom recurring scheduler (Telegram `schedule_date` only; no cron)
- TUI / farm control plane
- MCP server + agent skill (Phase 6, deferred to end of development) — MCP now `done`, skill still deferred (see `mcp` row in capabilities)
- Voice/video calls, secret chats, Passport, Stories as day-1
- Going public mid-build

### Non-functional requirements

| Area | Rule |
|------|------|
| Persistence | One SQLite `.session` per named account, stored outside CWD, never shared across processes |
| Rate limits | Sequential default; `--parallel` 1–32 with per-account token buckets (see ADR-008); honour `FloodWaitError` / `SlowModeWaitError` |
| Secrets | `TELE_API_ID` / `TELE_API_HASH` in `.env` (app-level, outside repo); never logged |
| Security | 2FA, QR login supported; `sign_out` deletes session |
| Proxy | Global + per-account SOCKS5 override (grammers 0.10 socks5-only) |
| Output | Human tables on stdout; JSON/JSONL on stdout for agents; logs on stderr only |
| Testing | `--dry-run` on all commands; unit + contract + selection test suites offline |

## Consequences

- Every row in the capability matrix that is `never` in v1 should stay `never`
  unless the product scope is re-evaluated.
- MCP now `done` (shipped as `tele mcp`); agent skill remains deferred (Phase 6) — see matrix `Explicitly later / never` section.
- The out-of-scope list is a living contract; flip a row to `later` only via a
  matrix update in the same commit that ships the capability.
