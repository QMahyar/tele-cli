# Wayfinder map: tele-cli optimization pass (10-agent review)

Label: `wayfinder:map` (local-markdown tracker; no remote tracker configured)

## Destination

Execute the four optimization slices from the 10-agent review, in order:
1 → 2 → 4 → 3, each on its own git branch off `main`, merged back after
clippy + `cargo test` pass. "Do the work" mode — this effort carries
execution in its Notes (wayfinder override), so tickets are task tickets,
not decision tickets.

## Notes

- Repo: E:\Code\Tele-Cli. Rules: AGENTS.md (no comments in code, clippy -D warnings,
  cargo test ~1475 offline, commit prefix `feat|fix|refactor|test|docs|chore:`).
- Branch naming: `slice/1-security`, `slice/2-correctness`, `slice/4-architecture`,
  `slice/3-features`. No worktrees — one checkout, sequential slices.
- Delegate to subagents in parallel where file sets are disjoint; subagents edit
  but do NOT commit; the driver verifies + commits.
- Every slice must leave `cargo test` + `cargo clippy --all-targets -- -D warnings` green.

## Decisions so far

(empty — effort starts here)

## Not yet specified

- Slice 3 (features) exact scope is large; ticketed per feature after slices 1-2
  land so branch bases are stable.

## Out of scope

- Anything from the review outside the 4 chosen slices (interface doc drift is
  folded into slice 2 verification; ecosystem/crates.io work is not in this pass).
