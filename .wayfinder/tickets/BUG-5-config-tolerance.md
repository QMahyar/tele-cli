# BUG-5: Config tolerance — unknown account keys + empty proxy table

**Source:** deep-dive auth/session review C2 + C4.

## Question/Problem

1. `write_config` (`src/config.rs:334-341`) replaces the whole `accounts` table from the struct — unknown keys INSIDE `[accounts.X]` are silently deleted on every rewrite (top-level unknowns are already preserved via toml_edit). Silent user-data loss.
2. `proxy_url_for` (`src/config.rs:295-299`): present-but-empty `[accounts.X.proxy]` table yields `Some(empty)` → hard "host must not be empty" error instead of falling back to global proxy or disabling.

## Acceptance criteria

- [ ] Unknown per-account TOML keys survive add/remove/rewrite round-trip (approaches: `#[serde(flatten)] extra` map on AccountConfig, or key-by-key merge with toml_edit — pick one, test round-trip incl. comments preservation which is already contract).
- [ ] Empty per-account proxy table falls back to global proxy config; if none, proxy disabled — no error. Explicit non-empty per-account proxy still overrides global.
- [ ] Unit tests: round-trip with unknown keys; empty-proxy fallback matrix (empty+global / empty+no-global / explicit+global).
- [ ] Existing config tests (incl. proptests) pass.

## Verification

- [ ] clippy -D warnings / fmt check / full `cargo test`

## Files

- `src/config.rs`, tests

## Constraints

Branch `fix/bug-5-config-tolerance`. Do not bump toml/toml_edit versions. No comments.
