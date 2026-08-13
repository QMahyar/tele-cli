# Tele-Cli Specification

**Version:** 1.0  
**Status:** Spec (locked)  
**Date:** 2026-08-13

## 1. Overview

Tele-Cli is a long-lived CLI tool that gives you and your AI agents native-equivalent control of many real Telegram phone accounts.

It exposes the **full user-client surface** of Telethon (not bots) so you can do anything a native user can: join channels, send scheduled messages, forward, edit, delete, listen to updates, etc.

The hard product is a **session kernel** (named accounts, persistent `.session` files, flood-aware executor). The human skin is a gh-style command-line interface. The development spine is a **living capability matrix** in-repo.

## 2. Scope

### In Scope
- Full Telethon user-client surface (friendly methods + raw `functions.*`)
- Named accounts with persistent sessions
- Sequential default, parallel optional with rate-limit throttling
- Human CLI (`tele` binary) + `--json` / JSONL for agents
- Stay-connected listen mode (filtered events)
- Raw TL access (`tele raw <method> ...`)
- Living capability matrix in-repo (Telegram → Telethon → CLI → status)

### Out of Scope (v1)
- Bot-token-first product (you asked for phone accounts)
- Custom recurring scheduler (Telegram `schedule=` only)
- TUI / farm control plane
- Shipping MCP + agent skill before CLI core (MCP last)
- Voice/video calls, secret chats, Passport, Stories as day-1
- Going public mid-build

## 3. Key Features (Capability Matrix)

The matrix is the single source of truth. Every slice must update or mark rows.

### Auth & Login
| Capability | Telethon Path | CLI Command | Status |
|------------|---------------|-------------|--------|
| Phone code request | `AuthMethods.send_code_request` | `tele account login` | **want** |
| 2FA / password | `AuthMethods.sign_in` + `edit_2fa` | Same | **want** |
| QR login | `AuthMethods.qr_login` + wait | Same | **want** |
| Logout + delete session | `AuthMethods.log_out` | `tele account logout` | **want** |
| Passkeys | `AuthMethods.qr_login` + new paths | Same | **later** |

### Messages & Media
| Capability | Telethon Path | CLI Command | Status |
|------------|---------------|-------------|--------|
| Send text/media | `MessageMethods.send_message` | `tele msg send` | **want** |
| Edit / delete / forward | `MessageMethods.*` | `tele msg edit`, `delete`, `forward` | **want** |
| Schedule send | `MessageMethods.send_message(schedule=...)` | `tele msg send --schedule ...` | **want** |
| Pin / unpin | `MessageMethods.pin_message` | `tele msg pin` | **want** |
| Send file / upload | `UploadMethods.send_file` + `upload_file` | Same | **want** |
| Download media | `DownloadMethods.download_media` | `tele msg download` | **want** |
| Reactions | `MessageMethods.send_reaction` | `tele msg react` | **want** |
| Polls | `MessageMethods.send_poll` | `tele msg poll` | **want** |
| Stories | `stories.sendStory` | `tele story send` | **later** |

### Chats & Groups
| Capability | Telethon Path | CLI Command | Status |
|------------|---------------|-------------|--------|
| Join / leave channel | `channels.JoinChannel`, `LeaveChannel` | `tele chat join`, `leave` | **want** |
| Invite links | `messages.ExportChatInvite` | `tele chat invite` | **want** |
| Forums / topics | `channels.CreateForumTopic` | `tele topic create` | **want** |
| Participants / kick / admin rights | `ChatMethods.*` | `tele chat participants`, `kick` | **want** |
| Admin log | `ChatMethods.iter_admin_log` | `tele chat adminlog` | **want** |
| Stats | `ChatMethods.get_stats` | `tele chat stats` | **want** |

### Updates & Listen
| Capability | Telethon Path | CLI Command | Status |
|------------|---------------|-------------|--------|
| All updates (filtered) | `client.on(...)` + `run_until_disconnected` | `tele listen --events ...` | **want** |
| Filterable events | All event builders | Same | **want** |
| Auto-reconnect | `auto_reconnect=True` | Same | **want** |

### Account & Misc
| Capability | Telethon Path | CLI Command | Status |
|------------|---------------|-------------|--------|
| Dialogs / drafts / folders | `DialogMethods.*` | `tele dialog list`, `drafts`, `archive` | **want** |
| Contacts / block / privacy | `contacts.*`, `account.*` | `tele contact`, `block` | **want** |
| Profile / emoji status / accent colors | `account.*`, `help.getPeerColors` | `tele profile` | **want** |
| Takeout | `account.takeout` | `tele takeout` | **want** |
| Raw TL methods | `client(functions.*)` | `tele raw <method> ...` | **want** |

## 4. Non-Functional Requirements

- **Persistence:** One SQLite `.session` per named account, stored outside CWD, never shared across processes.
- **Rate Limits:** Sequential default; parallel ≤ 3; honor `FloodWaitError` / `SlowmodeWaitError` (Telethon default `flood_sleep_threshold=60`).
- **Secrets:** `TELE_API_ID` / `TELE_API_HASH` in `.env` (app-level).
- **Security:** 2FA, QR login supported. `log_out()` deletes session.
- **Concurrency:** Sequential default; `--parallel N` (N≤3).
- **Proxy:** None by default; global + per-account override (SOCKS and MTProto).
- **Output:** Human tables on stdout; JSONL on stdout for agents; logs on stderr; `-q` / `-v`.
- **Testing:** `--dry-run` on all commands; unit tests for kernel.
- **Upstream:** Telethon pinned (`>=1.44`) with manual `uv lock --upgrade`.

## 5. Tech Stack

- Python 3.11+
- Telethon (pinned)
- Typer (CLI)
- Rich (tables)
- pydantic-settings (config)
- `tele raw` and MCP (last)
- Sessions: SQLite (default), configurable

## 6. Acceptance Criteria

- Matrix is maintained and up-to-date
- `tele account login` works end-to-end (code/2FA/QR)
- `tele msg send` works with `--schedule`
- `tele listen` streams NewMessage (filtered)
- `tele raw <method>` and `tele mcp` expose full surface
- No silent skipping of Telegram capabilities

**Next phase:** Planning-and-task-breakdown → break the matrix into verifiable slices.

Ready for planning? Or any row to flip first?