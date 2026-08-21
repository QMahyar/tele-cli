# BUG-6: Stale cached access_hash — evict, retry, hint

**Source:** deep-dive plumbing review Task C.

## Question/Problem

Numeric-ID targets trust cached access_hash indefinitely (`src/entities.rs:78-90`). When the hash goes stale (user deleted/recreated, channel migrated), `resolve_peer` fails with raw server RPC text; the actionable `PEER_UNKNOWN_HINT` fires only for `Dropped`, not for `PEER_ID_INVALID`/`CHANNEL_INVALID` after a cache hit.

## Acceptance criteria

- [ ] On cached-ref resolve failure matching PEER_ID_INVALID / CHANNEL_INVALID / PEER_ID_INVALID-family codes: evict cached ref, retry resolution once via existing fallback probe path.
- [ ] If retry also fails, surface error with `PEER_UNKNOWN_HINT` appended (same UX as Dropped path).
- [ ] Corrupt cache row read failure logs `log::warn!` instead of silent `.ok()` swallow (`entities.rs:188,232`).
- [ ] Unit tests: stale-hit → evict → retry → hint flow using seams/fixtures; warn-on-corrupt-row test.

## Verification

- [ ] clippy -D warnings / fmt check / full `cargo test`

## Files

- `src/entities.rs`, tests

## Constraints

Branch `fix/bug-6-stale-cache`. Do not add TTL/expiry policy (scope: failure-triggered invalidation only). No comments.
