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
- Freeform lines follow their own rule. `[info]`, `[warn]`, and `[error]` lines print by default. The freeform floor tracks the effective log level: at `TELE_LOG=warn` only `[warn]` and `[error]` freeform lines print; at `TELE_LOG=debug` or `-vv` all levels pass. `-q` sets the floor to `[error]` only. `-v`/`-vv` and `TELE_LOG` raise or lower the floor the same way they affect `log`-crate lines.
- Every freeform line has the shape `[level] message`. There are no structured events and no `run_id`. Correlate events by reading the whole stderr stream.
- In human mode with stderr attached to a terminal, `takeout export` writes progress to stderr. Lines look like `[info] dialogs page 1: +21 dialogs` and `[info] dialog 3/57 Alice msgs=120`. Machine output stays untouched. Machine mode (`--json`/`--jsonl`), piped or redirected stderr, and `-q` all silence progress.
- Color policy: tele emits no ANSI color on stdout or stderr today (table cells are stripped of escapes, and the logger never adds any), so `NO_COLOR` (present with any value, per no-color.org) and `TERM=dumb` are honored by construction. No color helper ships until the first emission site exists — when one does, it must gate on `NO_COLOR` presence and `TERM=dumb`. There is no `--no-color` flag (stdout coloring stays out of scope while output is monochrome; a flag would need a `main.rs` change owned elsewhere).
- Levels are typed in code (`output::LogLevel`: `error` > `warn` > `info` > `debug`, parsed case-sensitively). `output::log_line(&str, _)` remains as a compatibility shim over the typed `output::log_level` and keeps its unknown-level diagnostic; new call sites use the enum.
- No log line ever carries api_hash, session data, phone numbers, passwords, QR login tokens, or the full `--args` value of `tele raw`.

## What gets logged

- `[info]` freeform lines report lifecycle notices: what a dry run would do, login and logout and remove outcomes, listen start and timeout.
- `[error]` freeform lines report failures: one line per failed account with the account name and error text, listen reconnect attempts, and top-level command errors.
- When you set `TELE_LOG`, the `log` crate adds its own `[LEVEL] message` lines. At `trace`, these include grammers internals, which bypass the secret scrubbing in the next rule: the CLI refuses `TELE_LOG=trace` unless you acknowledge the leak risk with `--allow-trace` (or `TELE_ALLOW_TRACE=1`). Treat all stderr captured at `trace` as sensitive and never paste it into a ticket or a log.

## Alerting

Nothing pages anyone. Tele-Cli runs locally and ships no alerting integration. You detect a failure through a non-zero exit code plus an `error` object in the stdout envelope.

## Deliberate gaps (sized, no implementation)

- `log` stays; no `tracing` migration. The four on-call questions above are answerable from envelopes plus freeform lines, and every error line already carries its account name. A migration would add two runtime dependencies (`tracing`, `tracing-subscriber`) for span correlation a short-lived CLI process does not need. Revisit only if `--parallel` fan-out debugging outgrows the per-account error lines.
- `TELE_LOG` stays; `RUST_LOG` is not honored. `TELE_LOG` (`trace|debug|info|warn|error`) plus `-v`/`-vv`/`-q` is the product contract documented above; honoring `RUST_LOG`/`EnvFilter` in parallel would create two sources of truth for one level. Operators coming from other tools should set `TELE_LOG`.
- No `run_id` / structured stderr events. One-shot commands correlate through the envelope (`command` plus per-account rows); adding a run id would be a purely additive future change, not a fix.
- No span instrumentation on the fan-out, no dynamic level reload, no metrics/OTLP. The process is not a service: there is no long-lived task to re-level, and the monitoring API is exit codes plus envelopes. These stay absent until a daemon mode exists (out of scope).
