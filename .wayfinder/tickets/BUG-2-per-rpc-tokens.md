# BUG-2: Per-RPC tokens for paginated iteration

**Source:** deep-dive dialog-group H1 + msg review (get loop).

## Question/Problem

One `rate_limiter.acquire()` covers an entire iteration loop, but grammers fetches pages of ~100 per RPC:
- `src/commands/dialog.rs:78` and `:171` — `iter_dialogs()` loop
- `src/commands/privacy.rs:119` — privacy get (9 RPCs under 1 token)
- `src/commands/msg.rs:908` — `msg get` acquires once before the while-next loop

A `--limit 10000` run fires ~100+ unthrottled RPCs — violates ADR-008 per-account token-bucket gating.

## Acceptance criteria

- [ ] Token acquisition scales with fetched pages, not command count: acquire per page/chunk inside each loop (or a small helper wrapping "acquire N tokens per page").
- [ ] Applied at all four sites (dialog list, dialog drafts path if it iterates, privacy get, msg get).
- [ ] Existing tests pass; add a unit test proving token demand grows with limit (seam/mock counter).
- [ ] No behavior change for small limits (first page already covered by existing single acquire).

## Verification

- [ ] clippy -D warnings / fmt check / full `cargo test`

## Files

- `src/commands/dialog.rs`, `src/commands/privacy.rs`, `src/commands/msg.rs`, tests

## Constraints

Branch `fix/bug-2-per-rpc-tokens`. Touch only the loop regions; do not restructure handlers. No comments.
