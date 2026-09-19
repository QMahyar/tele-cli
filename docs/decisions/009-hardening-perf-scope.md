# ADR-009: Hardening and performance scope

## Status

Accepted

## Date

2026-09-19

## Context

Hardening and optimisation proposals re-litigated scope, posture, and proof
from scratch: no shared taxonomy of what counts as an improvement, no
ordering rubric, and no boundary against the review-remediation effort
(validator/core/dry-run agreement), which owns its own backlog.

## Decision

- Five-way taxonomy: (1) correctness leftovers beyond review-remediation,
  (2) hardening, (3) performance, (4) UX/operability follow-ups,
  (5) docs/harness trust.
- Ordering rubric: risk × payoff × reversibility (1–3 each), highest
  risk-reduction per effort first. Hard gates: additive `--json`/JSONL
  only (posture acks exempted per the Stability section of
  `docs/cli-contract.md`), no new runtime dependency without asking,
  offline tests, clippy + test green. Irreversible posture changes need
  explicit sign-off regardless of score.
- Boundary: review-remediation owns validator/core/dry-run agreement; this
  effort owns net-new improvements plus re-audit deltas, and never
  double-owns a resolved remediation item except via its delta.

## Alternatives considered

### Payoff-first ordering

Rejected: a user-visible speedup that risks account safety (e.g.
intra-account page-walk concurrency against FloodWait escalation) must lose
to risk-reduction until measured.

### Double ownership where justified

Rejected: two backlogs owning one item produced the re-litigation this ADR
exists to stop; deltas reference the owning item instead.
