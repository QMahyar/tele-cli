# Observability

This page lists every signal the CLI emits while a command runs. You can answer four on-call questions from these signals without reading source code.

## On-call questions

1. Which account ran which command, and did it succeed?
2. Did Telegram return FloodWait or SlowModeWait, and for how many seconds?
3. Did the CLI fail to lock or connect a session?
4. During `tele listen`, is the connection alive, and which event types flow?

## Signals

| Question | Signal | Where |
|---|---|---|
| 1 | Per-account `ok` and `error` fields under `results[].account` | stdout envelope with `--json` |
| 2 | `error.seconds` on a FloodWait or SlowModeWait | stdout envelope with `--json` |
| 3 | An `[error]` line from the failing command | stderr |
| 4 | `[info]` lifecycle lines, such as `listen streams JSONL events on stdout`, plus one `[error]` line per failed account stream | stderr for lifecycle lines, stdout for the event rows themselves |

RPC failures carry two additive JSON keys. `error.code` holds the numeric Telegram error code. `error.name` holds the TL error name, for example `FLOOD_WAIT`. Match automation on these two keys rather than on `message`, because freeform text can change between versions.

When peer resolution fails, the error tells you to refresh the peer cache. It does not surface the bare grammers text `dropped (cancelled)`.

## Log rules

- The CLI writes all logs to stderr. stdout carries tables or machine output only.
- The `log` crate is off by default. No `[LEVEL] message` lines appear until you enable it. Set `TELE_LOG` to `trace`, `debug`, `info`, `warn`, or `error` to enable logging and select the level.
- With `TELE_LOG` unset, `-v` selects INFO and `-vv` selects DEBUG. `-q` selects ERROR and overrides both `TELE_LOG` and `-v`.
- Freeform lines follow their own rule. `[info]`, `[warn]`, and `[error]` lines print by default, and with `-q` only `[error]` freeform lines print. This channel ignores `TELE_LOG` and the `-v` flags entirely.
- Every freeform line has the shape `[level] message`. There are no structured events and no `run_id`. Correlate events by reading the whole stderr stream.
- In human mode, `takeout export` writes progress to stderr. Lines look like `[info] dialogs page 1: +21 dialogs` and `[info] dialog 3/57 Alice msgs=120`. Machine output stays untouched.
- No log line ever carries api_hash, session data, phone numbers, passwords, QR login tokens, or the full `--args` value of `tele raw`.

## What gets logged

- `[info]` freeform lines report lifecycle notices: what a dry run would do, login and logout and remove outcomes, listen start and timeout.
- `[error]` freeform lines report failures: one line per failed account with the account name and error text, listen reconnect attempts, and top-level command errors.
- When you set `TELE_LOG`, the `log` crate adds its own `[LEVEL] message` lines. At `trace`, these include grammers internals.

## Alerting

Nothing pages anyone. Tele-Cli runs locally and ships no alerting integration. You detect a failure through a non-zero exit code plus an `error` object in the stdout envelope.
