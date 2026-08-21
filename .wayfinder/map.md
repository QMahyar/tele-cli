# Wayfinder Map — review-findings

Label: wayfinder:map
Tracker: local markdown (`.wayfinder/tickets/`)
Status: EXECUTION-IN-MAP (user override — tickets carry fixes to done, not just decisions)

## Destination

All confirmed findings from the 5-agent codebase review fixed on integration branch `fix/review-findings`: fmt/clippy/tests green, machine contract evolved additively only, docs synced. `main` untouched; user reviews and merges.

## Notes

- Rust CLI, grammers 0.10. Read AGENTS.md before any ticket.
- Conventions: no comments in code; clippy `-D warnings` + fmt + full test suite green per ticket; commit prefixes `fix|refactor|test|docs|chore:`; never push; never touch `main`.
- Branch flow: each ticket branch cut from current head of `fix/review-findings`, merged back after gate. Sequential (single checkout, no worktrees).
- proptest approved as dev-dependency. CI edits approved. New runtime deps NOT approved.

## Decisions so far

<!-- one line per closed ticket -->

## Not yet specified

- macOS CI job: release.yml ships mac binaries but ci.yml never tests the target — needs runner/budget decision beyond this effort's approved CI scope.

## Out of scope

- raw.rs registry developer-facing error message ("add an arm in src/commands/raw.rs") — text is mandated verbatim by docs/cli-contract.md; changing it is a contract-major concern, not a fix.
