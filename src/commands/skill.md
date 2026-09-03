---
name: tele
description: Drive real Telegram user accounts through the `tele` CLI (MTProto user sessions, no bot tokens). Use when sending, reading, editing, forwarding, or deleting messages; managing chats, dialogs, forum topics, contacts, privacy, or stories; streaming live updates; running multi-account automation; or exposing Telegram to an MCP client. Triggers: "send a telegram message", "check my telegram", "read telegram chat", "stream telegram events", "telegram automation".
license: MIT
compatibility: tele 0.8.0+
---

# tele — Telegram user-account CLI

## Non-negotiable rules

1. Parse only `--json` or `--jsonl` output. Human tables are not an API.
2. Read data from stdout only. stderr carries logs.
3. Run destructive commands with `--dry-run` first, then for real.
4. Exit codes: `0` ok, `1` usage (fix the command, do not retry), `2` partial, `3` all failed, `4` auth (run `tele account login --name NAME`), `130` interrupted.
5. One process per account. `session is in use` means another tele process holds the account's session lock; wait for it or stop it. Never share or copy session files between machines.
6. Secrets (api_hash, phone numbers, 2FA passwords) never appear in output. Do not read or echo session file contents.
7. Destructive commands (`msg delete`, `chat kick`, `chat leave`, `dialog delete`, `account delete`, `topic delete`, `story delete`, `sticker remove`, `contact remove`) refuse to run without an explicit `--account` or `--tag`.

## Targeting

- Accounts: `--account NAME` (repeatable), `--tag TAG` (repeatable, union), `--parallel 1-32` (concurrency cap).
- Chats (`--chat` or `--target`): `@username`, `t.me/...` link, numeric id, `-100...` channel id, `me`, `+phone`.

## Command map

| group | does |
|---|---|
| `account` | list, add, login (code/QR), logout, remove, status, sessions, password, export-session, import-session, ttl, delete, phone |
| `msg` | send, get, edit, delete, forward, search, react, download, read, pin, vote, typing, click |
| `chat` | join, create, leave, participants, kick, admin, admin-log, stats, invite, requests, settings, edit, link |
| `dialog` | list, drafts, archive, unarchive, delete, pin, draft |
| `topic` | forum topics: list, create, close, reopen, edit, delete, pin |
| `contact` | list, add, remove, block, unblock |
| `profile` | get, set (name, bio, username), photo, emoji-status |
| `privacy` | get, set (14 rule keys) |
| `story` | send, list, read, delete, pin, unpin |
| `sticker` | list, search, show, install, remove |
| `takeout` | start, export, finish (account data export) |
| `listen` | stream live events as JSONL |
| `serve` | duplex JSONL server over stdin/stdout (1–32 accounts) |
| `mcp` | MCP stdio server: tele ops as tools (exactly one account) |
| `raw` | typed allowlist of 25 Telegram TL methods |
| `skill` | print or install this skill |

## Output envelope

Every one-shot command with `--json` emits one object:

```json
{"ok": true, "command": "msg send", "results": [{"account": "work", "ok": true, "data": {}, "error": null}]}
```

- Success means `ok: true` at the top and on every result you care about.
- `error.type`: `UsageError` (bad args), `AuthError` (re-login), `ConfigError` (bad config.toml/.env), `InvocationError` (Telegram refused; carries `code`/`name`), `Timeout`.
- `listen` and `serve` stream JSONL: one JSON object per line, forever (or until `--timeout-secs`). A `gap` row means updates were missed; resync if you need exactness.

## Recipes

```bash
tele msg send --chat '@team' --text "hi" --json
tele msg send --tag bulk --chat '@chan' --text "x" --parallel 8
tele msg get --chat '@team' --json | jq '.results[0].data'
tele msg delete --chat '@team' --ids 123 --dry-run        # preview, then drop --dry-run
tele listen --events NewMessage --chat '@team' --jsonl   # stream; each line is one event
tele chat participants --chat '@group' --json
tele profile get --json
tele mcp --account work --read-only                      # MCP server for tool-calling clients
tele serve --account work                                # duplex JSONL: actions in, events out
```

## Reference

- `tele <group> --help` lists every command and flag.
- `docs/cli-contract.md` in the repo is the full machine contract (JSON shapes, exit codes, MCP tools, protocol rules).
- `tele skill install [--dir PATH]` installs this skill into detected agent skill directories.
