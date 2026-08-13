# ADR-002: Capability matrix is the development spine

## Status
Accepted

## Date
2026-08-13

## Context
“Full from start” meant “do not silently skip Telegram capabilities,” not “implement every TL method before first send.” Telegram’s method list is far larger than Telethon’s friendly client. Layers move (docs cite ~225).

## Decision
Keep `docs/capabilities.md` as DoD. Each row: Telegram path, Telethon path (friendly | raw | none), CLI, status `want|later|never|done`. Ship vertical slices that flip rows to `done`. Escape hatch: `tele raw`. On Telethon bump, diff Client Reference + layer changelog into the matrix.

## Alternatives considered

### Implement every TL method before publish
Rejected: never ships; stale the week Telegram adds a layer.

### Friendly methods only
Rejected: stories, forums, reactions, etc. often need `functions.*`.

## Consequences
- PRs that add commands must edit the matrix in the same change.
- Contract test should fail if a `want` row has no command/`tele raw` mapping.
