# CAP-14: Listen upgrades — Gap event, albums, DM deletions

**Effort:** M · **Deps:** none · **Branch:** `feat/cap-14-listen-upgrades`

## Goal

Consumers can't detect update loss; albums arrive as independent rows; DM/basic-group deletions never match under `--chat`.

## Acceptance criteria

- [ ] Synthetic `Gap` JSONL row emitted when grammers signals difference fetch / queue overflow (`update_queue_limit` hit) — includes state snapshot fields consistent with Raw rows.
- [ ] `--events Album`: consecutive NewMessage sharing non-null grouped_id coalesce into one Album row (messages array + shared metadata); timeout flush ~500ms after last member so stragglers don't hang the stream.
- [ ] DM deletion matching: local id→peer map for UpdateDeleteMessages without channel_id, enabling `--chat` filter on DM deletions; document memory bound.
- [ ] Document the previously-undocumented behavior change in docs/cli-contract.md (additive); offline tests with fixture updates incl. grouping timer logic (tokio time pause).
