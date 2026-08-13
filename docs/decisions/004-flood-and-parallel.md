# ADR-004: Sequential default, parallel max 3, flood_sleep 60

## Status
Accepted

## Date
2026-08-13

## Context
Fan-out join/send across many phones is the user’s first example and Telegram’s ban pattern. Telethon default `flood_sleep_threshold` is 60s; waits above that raise `FloodWaitError`. Official API uses `FLOOD_WAIT_X` / `SLOWMODE_WAIT_X`.

## Decision
Default `--parallel 1`. Clamp to 3. Let Telethon sleep ≤60s. Surface longer waits in JSON `error.seconds` and log `telegram_wait`. Do not implement a custom token bucket until measured.

## Alternatives considered

### High parallelism with jitter
Rejected: looks like spam; Telethon FAQ is explicit.

### Infinite auto-sleep
Rejected: a 24h wait would hang agents. Threshold stays 60.

## Consequences
- Partial success uses exit 2.
- Live tests must not fan-out joins unless disposable.
