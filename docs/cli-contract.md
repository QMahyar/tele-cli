# CLI contract

Public interface for humans and agents. Hyrum: `--json` shape and exit codes are commitments. Add fields; do not rename or remove without a major version. Stderr carries freeform `[level] message` lines only (see `docs/observability.md`), never machine output.

## Invocation

```
tele [GLOBAL] GROUP COMMAND [ARGS]

Globals (root callback, inherited):
  --account NAME     repeatable; NAME or all
  --tag TAG          repeatable; union with --account
  --parallel N       default 1; max 32 (values outside 1..=32 are clamped with a warning)
  --json             machine output on stdout
  --jsonl            machine output: JSON lines (one-shot commands emit a single envelope line; only `tele listen` emits one record per event)
  --quiet / -q
  --verbose / -v     maps to log level
  --dry-run
  --config PATH
```

Empty selection is an error except `tele account list|add`.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | All selected accounts succeeded (or dry-run) |
| 1 | Usage / validation (bad flags, unknown account, bad JSON args) |
| 2 | Partial: some accounts succeeded, some failed — or an account operation partially completed (e.g. `msg delete` removed fewer than requested) |
| 3 | All selected accounts failed (Telegram / IO) |
| 4 | Auth required (not logged in, 2FA needed and not supplied) |
| 130 | Interrupted (SIGINT) |

Do not overload 1 for Telegram errors.

## `--json` envelope (one-shot)

Stdout is **one JSON object** (pretty=false, UTF-8). No logs on stdout.

```json
{
  "ok": true,
  "command": "msg send",
  "dry_run": false,
  "results": [
    {
      "account": "work",
      "ok": true,
      "data": {},
      "error": null
    }
  ]
}
```

`error` when `ok` is false:

```json
{
  "type": "InvocationError",
  "message": "A wait of 17 seconds is required",
  "seconds": 17
}
```

`seconds` is present only for flood-wait errors (FLOOD_WAIT / SLOWMODE_WAIT, RPC 420) and carries the wait duration in seconds.

RPC-backed `InvocationError` errors additionally carry the raw Telegram RPC
identity as additive keys:

```json
{
  "type": "InvocationError",
  "message": "rpc error 400: CHAT_INVALID",
  "code": 400,
  "name": "CHAT_INVALID"
}
```

`code` and `name` are present only when the failure maps to a Telegram RPC
error; scripts should match on these instead of parsing `message`.

Pre-flight failures (usage validation, config load/parse, account selection)
happen before any account runs. In `--json`/`--jsonl` mode they still emit one
envelope on stdout: `ok: false`, empty `results`, and a top-level `error`
object with the same fields as `results[].error`:
Clap parse errors (unknown subcommand, missing required flag) and the
`--json`/`--jsonl` conflict also emit this envelope on stdout when `--json` or
`--jsonl` is present.

`--jsonl` for one-shot commands is identical to `--json`: exactly one envelope
line on stdout (valid JSONL). Only `tele listen` emits one record per event
(see below). Envelope `error.message` strings are stripped of ANSI escapes.

```json
{
  "ok": false,
  "command": "account list",
  "dry_run": false,
  "results": [],
  "error": {
    "type": "ConfigError",
    "message": "failed to parse C:\\path\\config.toml: ..."
  }
}
```

Rules:

- `data` is additive per command. Document new keys in this file when added.
- `account list --json` also emits a top-level `accounts` key: an array of the
  same rows as `results[].data` (each `{"name","tags","session"}`). It duplicates
  the data — consumers should prefer `results`.
- Telegram objects are serialized via an allowlist (`id`, `date`, `message`, `peer`, …). Never dump raw `api_hash`, session, or auth keys.
- `--dry-run`: `ok=true`, `dry_run=true`, no network. Every dry-run
  `results[].data` envelope carries `dry_run`, a human-readable `would`
  describing the exact intended action (using the command's argument values),
  and the command's own argument keys — all additive. `account add` and
  `tele listen` follow the same `would` convention where applicable.

Human mode (no `--json`): Rich tables on stdout. Same exit codes.

## `msg delete`

`results[].data` carries `requested` (number asked to delete) and `deleted`
(number actually removed server-side). When `deleted < requested` (already-deleted
ids, others' messages, no permission) the row also carries `"partial": true` and
the process exits 2. `--self-only` deletes only for yourself (private chats and
basic groups; rejected for channels) via `messages.deleteMessages { revoke: false }`.
Mutually exclusive with `--all`.

## Listen / stream

`tele listen` always streams **JSON Lines** on stdout, one event per line; `--json`
is accepted as a no-op for symmetry. Stdout writes are backpressured — `listen`
pauses on a slow reader instead of dropping events:

```json
{"event":"NewMessage","account":"work","id":123,"chat_id":456,"text":"...","date":"2026-08-13T12:00:00+00:00"}
```

`Raw` rows (from `--events Raw`, or `--raw` which implies it) carry the raw update
base64-encoded in a `raw` field plus a `state` object with `date`/`seq` and, per
the message-box variant, `pts` (common/channel box), `qts` (secondary box), or
`channel_id` + `pts` (channel box):

```json
{"event":"Raw","account":"work","raw":"<base64 TL serialization>","state":{"date":123,"seq":456,"pts":42}}
```

Default event type: `NewMessage` only. `--events` is an allowlist that gates all
rows, including `Raw`. Unknown event names → exit 1 before connect.

`tele listen --dry-run --json`/`--jsonl` emits one JSONL row per selected
account describing the intended stream, following the `would` convention
(`event` holds the configured event allowlist, comma-joined):

```json
{"event":"NewMessage","account":"work","dry_run":true,"would":"stream NewMessage updates from account work"}
```

## `tele raw`

```
tele raw TL_NAME --args JSON
```

`TL_NAME` is a **registry name** from `src/commands/raw.rs` (e.g.
`messages.GetAllDrafts` with `--args '{}'`, `contacts.Search` with
`--args '{"q":"alice"}'`, `messages.ExportChatInvite` with
`--args '{"chat":"@mychat"}'`).
Rust TL types are static, so the registry is a typed match: each supported method
has a handler arm and documented `--args` shape. Unregistered names exit 1 with
the message `raw method not in registry; add an arm in src/commands/raw.rs`.
`--args` is a JSON object of constructor kwargs. Result goes in `results[].data`.
Destructive raw calls still require `--account` and honor `--dry-run` (dry-run does
not invoke).

## `tele completions`

```
tele completions bash|zsh|fish|powershell
```

Prints shell completion script for the `tele` binary to **stdout**, exits 0.
No account selection or network involved.

## Stability

- New commands and new optional JSON keys = MINOR.
- Exit code meaning change, renamed JSON keys, or removed commands = MAJOR.
- Changelog is consumer-facing (`CHANGELOG.md`), not `git log`.
