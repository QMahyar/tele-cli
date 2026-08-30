# ADR 006: Rust CLI with grammers (drop Python)

- Status: accepted
- Date: 2026-08-13
- Supersedes: ADR 001 (session kernel stack assumptions), spec stack table

## Context

The project was originally spec'd as Python 3.11 + Telethon + Typer. The user
requires a Rust codebase. Telethon is Python-only, so "Rust + Telethon" is
impossible. Options considered:

1. **Rust CLI + embedded Python kernel (subprocess/PyO3)** — keeps Telethon but
   reintroduces a Python runtime, contradicts "no Python in the repo".
2. **Pure Rust + grammers** — grammers is the Rust MTProto suite by Lonami,
   the same author as Telethon. Fully typed, same TL surface, same
   friendly-vs-raw split.

Chosen: **option 2, pure Rust + grammers** (`grammers-client`, `grammers-tl-types`,
`grammers-session`, `grammers-crypto`).

## Consequences

- Single static binary. No Python runtime, no `uv`, no pip, no virtualenv.
- The capability matrix's "Telethon" column becomes "grammers". Friendly API
  covers auth (code/2FA), messages (send/edit/delete/forward/pin/get/read/
  reactions/search), chats (join/leave/participants/kick/admin/banned),
  dialogs, files (upload/download), updates (`stream_updates`).
- **QR login has no friendly helper** in grammers: implement via raw
  `auth.exportLoginToken` + `updateLoginToken` handling in the update stream.
- **Scheduled sends have no friendly param**: raw `messages.SendMessage`
  with `schedule_date`.
- **`tele raw` cannot be fully dynamic**: TL types are static Rust enums, so
  `tele raw` uses a typed registry (match arm per supported TL function).
  Unregistered names fail with a clear error and a pointer to add an arm.
- Flood/slowmode handling: `ClientConfiguration` + `AutoSleep` retry policy
  (sleeps and retries once on flood). Executor still sequential default,
  `--parallel` clamped 1–32.
- Sessions: `grammers_session::FileSession` per account, one file per account,
  never two clients on one file.
