# ADR-005: No public release until want-matrix is done

## Status
Accepted

## Date
2026-08-13

## Context
User will maintain the tool long-term and does not want a half-client published. MCP and agent skill are explicitly last.

## Decision
No PyPI/GitHub Release until `docs/capabilities.md` has no `want` rows (MCP/skill remain `later`). Versioning, CI, and changelog process still exist from day one (`docs/release.md`) so publish is mechanical.

## Alternatives considered

### Publish 0.x early
Rejected by product intent.

## Consequences
- CI still runs on every PR.
- `0.1.0` may exist locally; public upload is gated.
