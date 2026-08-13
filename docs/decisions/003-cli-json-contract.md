# ADR-003: Stable CLI + JSON envelope

## Status
Accepted

## Date
2026-08-13

## Context
Humans and agents share one binary. Agents will depend on whatever `--json` emits (Hyrum). Typer `--help` is not a sufficient contract.

## Decision
Document exit codes and a single envelope (`ok`, `command`, `dry_run`, `results[]`) in `docs/cli-contract.md`. Listen uses JSONL. Additive keys only; breaking changes are MAJOR.

## Alternatives considered

### Ad-hoc prints / different JSON per command
Rejected: agents cannot share parsers.

### MCP-only machine API
Rejected: MCP is last; CLI must be agent-usable now.

## Consequences
- Serialize through an allowlist.
- Change the envelope only via changelog + semver.
