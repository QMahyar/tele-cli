# Observability

On-call questions this CLI must answer without reading source:

1. Which account ran which command, and did it succeed?
2. Did Telegram flood/slowmode us, and for how many seconds?
3. Did we fail to lock or connect a session?
4. (listen) Are we connected, and which event types are flowing?

## Signals

| Question | Signal | Where |
|---|---|---|
| 1 | log `command_finished` | stderr JSON or key=value |
| 2 | log `telegram_wait` + JSON `error.seconds` | stderr + stdout `--json` |
| 3 | log `session_lock` / `connect_failed` | stderr |
| 4 | log `listen_started` / `listen_event` (counts, not bodies at info) | stderr |

No Prometheus in v1. Structured logs are enough for a local CLI.

## Log rules

- Destination: **stderr only**. stdout is tables or `--json`.
- Default level: WARNING. `-v` → INFO. `-vv` / `--verbose` twice → DEBUG. `-q` → ERROR.
- Shape: `event=<name>` plus fields. Prefer JSON logs when `--json` so agents can ignore stderr or parse it.
- Correlation: `run_id` (uuid4 per process invocation) on every line.
- Cardinality: fields from small sets (`account`, `command`, `event`, `ok`). Message text only at DEBUG, truncated.
- Never: api_hash, session, phone, password, full `--args` for raw at INFO.

## Event names (stable)

| event | level | fields |
|---|---|---|
| `command_started` | info | run_id, command, accounts, dry_run, parallel |
| `command_finished` | info | run_id, command, ok, failed_count |
| `telegram_wait` | warn | run_id, account, seconds, kind=flood\|slowmode |
| `session_lock` | error | run_id, account, path (basename only) |
| `connect_failed` | error | run_id, account, type |
| `listen_started` | info | run_id, account, events |
| `listen_reconnect` | warn | run_id, account |

## Alerting

None paged. This is a local tool. Symptom the user feels: non-zero exit + `error` object.
