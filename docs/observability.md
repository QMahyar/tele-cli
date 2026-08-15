# Observability

On-call questions this CLI must answer without reading source:

1. Which account ran which command, and did it succeed?
2. Did Telegram flood/slowmode us, and for how many seconds?
3. Did we fail to lock or connect a session?
4. (listen) Are we connected, and which event types are flowing?

## Signals

| Question | Signal | Where |
|---|---|---|
| 1 | per-account `ok` / `error` in the stdout envelope | stdout `--json` |
| 2 | `error.seconds` on flood/slowmode wait | stdout `--json` |
| 3 | `[error] ...` freeform stderr line from the failing command | stderr |
| 4 | `[info]` listen lifecycle lines (`listen streams JSONL events on stdout`, `listen timeout reached`, per-account stream failures) | stderr + stdout JSONL rows |

No Prometheus in v1. Two freeform stderr channels are enough for a local CLI.

## Log rules

- Destination: **stderr only**. stdout is tables or `--json`.
- The `log` crate is **Off by default** (no `[LEVEL] message` lines at all).
  `TELE_LOG=trace|debug|info|warn|error` enables it and selects the level.
  With `TELE_LOG` unset, `-v` → INFO, `-vv` → DEBUG, `-q` → ERROR.
- Freeform `log_line` lines (`[info] ...`, `[warn] ...`, `[error] ...`) default to a
  minimum of INFO; `-q` raises the floor to ERROR. They are independent of
  `TELE_LOG` and of the `-v` flags.
- Shape: freeform `[level] message`. There are **no structured events and no
  `run_id`**; correlate by reading the whole stderr stream.
- Never: api_hash, session, phone, password, full `--args` for raw at INFO.

## What actually gets logged

- `[info]` freeform lines: lifecycle notices (dry-run would-do, login/logout/
  remove results, listen start/timeout).
- `[error]` freeform lines: per-account failures (account name + error message),
  listen reconnect attempts, top-level command errors.
- `TELE_LOG`-gated `[LEVEL] message` lines from the `log` crate (grammers
  internals at `trace`).

## Alerting

None paged. This is a local tool. Symptom the user feels: non-zero exit + `error` object.