# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

## Before exploring, read these

- **`AGENTS.md`** at the repo root (rules file, always loaded whole).
- **`docs/capabilities.md`**: capability matrix, one slice at a time (see `AGENTS.md` context hierarchy).
- **`docs/cli-contract.md`**: CLI JSON contract for the slice being touched.
- **`docs/decisions/`**: ADRs that touch the area being worked in (this repo uses `docs/decisions/`, not `docs/adr/`).
- **`docs/security.md`**, **`docs/observability.md`**, **`docs/release.md`**: only the section relevant to the task.

There is no `CONTEXT.md` or `CONTEXT-MAP.md` yet. If a needed concept has no settled term, proceed with repo vocabulary (`docs/capabilities.md` capability ids, CLI contract terms like envelope, fan-out, dry-run `would`) and note the gap for `/domain-modeling`.

## File structure

Single-context repo:

```
/
├── AGENTS.md
├── docs/
│   ├── capabilities.md
│   ├── cli-contract.md
│   ├── decisions/          ← ADRs (001 session kernel, 002 capability matrix, 003 CLI JSON contract, 004 flood and parallel superseded by 008, 005 release gate, 006 Rust/grammers pivot, 007 product scope v1, 008 per-account flood weights)
│   ├── security.md
│   ├── observability.md
│   └── release.md
├── src/
└── tests/
```

## Use the glossary's vocabulary

When output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `docs/capabilities.md` / `docs/cli-contract.md`. Do not drift to synonyms those docs explicitly avoid.

If the concept needed is not settled yet, that is a signal: either reconsider invented language or note it for `/domain-modeling`.

## Flag ADR conflicts

If output contradicts an existing ADR in `docs/decisions/`, surface it explicitly rather than silently overriding:

> _Contradicts ADR-007 (product scope v1), but worth reopening because…_
