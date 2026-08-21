# CAP-5: Global search

**Effort:** S · **Deps:** CAP-1 (uses enriched message JSON) · **Branch:** `feat/cap-5-global-search`

## Goal

`search_all_messages` exists unused — per-chat search only today.

## Acceptance criteria

- [ ] `tele msg search --global --query <q> [--limit N]` searches across all dialogs via `client.search_all_messages`; omitting `--global` keeps per-chat behavior.
- [ ] Rows reuse message JSON shaping; pagination/limit consistent with existing search.
- [ ] Offline tests for flag plumbing + row shaping; docs/cli-contract.md additive.
