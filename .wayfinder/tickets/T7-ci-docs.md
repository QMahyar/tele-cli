# T7 — ci/docs-sync

Labels: wayfinder:task
Branch: fix/t7-ci-docs
Blocked by: T6

## Question

CI has no supply-chain gate, floats its toolchain, and AGENTS.md understates the suite — what metadata makes this repo self-describing again?

## Scope (approved by user)

- ci.yml: add cargo-audit step (cargo install cargo-audit || cached; audit only), concurrency group cancelling superseded runs.
- Toolchain: rust-version = current stable minor in Cargo.toml; MSRV check job pinned to it (cargo check only, ubuntu).
- AGENTS.md: correct test count line (will exceed 640 after T6) and any stale claims surfaced by fixes (e.g., record_flood removal note under Gotchas/executor description if T4 removed it).
- docs/capabilities.md: verify rows touched by T1-T5 still accurate; update only if drifted.

## Done when

Workflow YAML valid; rust-version pinned; docs match reality; fmt/clippy/tests green.
