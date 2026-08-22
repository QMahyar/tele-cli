# W1-2: listen.filters — sender/direction/regex/multi-chat

**Branch:** `feat/w1-2-listen-filters` · **Files:** `src/commands/listen.rs` only · **Deps:** none

## Goal

Scriptable event filtering server-of-truth side: game scripts receive only relevant events without post-filtering.

## Acceptance

- [ ] `--from USER` (repeatable; name mirrors existing `msg search --from`): resolve via existing peer machinery; keep events whose sender matches any; resolution failure at startup = Usage exit 1 before connect
- [ ] `--in` / `--out`: direction filter on outgoing() flag; both allowed together = union is meaningless — make them mutually exclusive via clap `conflicts_with`
- [ ] `--pattern RE`: regex on text field; invalid regex = Usage exit 1; use `regex` crate ONLY if already a dependency — otherwise hand-roll a conservative matcher? NO: adding the dep requires manager approval; check Cargo.toml first and if absent, stop and report instead of adding it
- [ ] `--chat` becomes repeatable (`Vec<String>`), union of targets; preserve existing single-chat JSON shape (additive only)
- [ ] Filters compose AND-wise across dimensions, OR-wise within one dimension
- [ ] Offline unit tests per filter + composition; gates green (fmt/clippy/cargo test)

## Boundaries

Only `src/commands/listen.rs`. Do not touch serve.rs (it reuses listen helpers — keep their signatures stable or update call sites inside listen.rs only; report if signatures must change).
