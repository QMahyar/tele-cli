# Release and publish

Rust/grammers reality. Unpublished until `want` rows in `docs/capabilities.md` are
`done` — that gate is currently **met** (all rows `done`/`later`/`never`; MCP/skill
stay `later` until Phase 6), per [ADR-005](decisions/005-unpublished-until-want-done.md).
This document is the contract so we do not invent a second process later.

Open release-readiness work:

- **CI is not yet implemented** (plan below is the contract for when it lands).
- Current branch is `master`; trunk target is `main` (rename pending).
- No release tags exist yet; `CHANGELOG.md` has the first `Unreleased` entry.

## Versioning

SemVer. Version is **derived from git tags**, not hand-edited in files.

- `MAJOR` — exit-code meaning changes, `--json` keys removed/renamed, removed
  commands
- `MINOR` — new commands or additive JSON keys
- `PATCH` — fixes, no contract change

`Cargo.toml` `version` must match the tag (`0.1.0` for `v0.1.0`); bump it in the
release commit. The contract for what counts as breaking lives in
`docs/cli-contract.md` ("Stability" section).

## Git

- Trunk-based. `main` must always pass the offline gate (see CI below).
- Branches: `feature/*`, `fix/*`, `chore/*`, `docs/*`. Short-lived, deleted after
  merge. Never force-push `main`.
- Commits: `feat|fix|refactor|test|docs|chore:` prefix — one logical change per
  commit.
- Never commit `.env`, `*.session`, `*.session-journal`, phones, or api hashes
  (`.gitignore` covers these; `docs/` must also be scrubbed of real numbers).
- Cargo.lock is currently gitignored; for a binary crate it should be committed.
  Decide when CI lands.

## Changelog

`CHANGELOG.md` is curated (Added / Changed / Fixed / Deprecated / Removed /
Security). **Write the entry with the change**, not at release archaeology time —
a change without its entry is unfinished. It is consumer-facing: describe CLI
behavior, not `git log`.

## CI (not yet implemented — contract for when it lands)

On PR and push to `main`, in a fresh checkout:

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test` (default — **no network**, no Telegram; `tests/contract.rs` reads
   only local files)
4. No Telegram secrets in CI: never load `%APPDATA%/telecli/.env`, never create
   sessions. Live checks stay manual on a developer machine with real sessions.

## Publish (when you explicitly say so)

1. Matrix gate: no remaining `want` rows (currently met) — or you explicitly waive
   named ids.
2. `CHANGELOG.md` has a version section for the tag; `Cargo.toml` version matches.
3. Tag `vX.Y.Z` **annotated** (`git tag -a -m "vX.Y.Z"`) and push tags.
4. Build: `cargo build --release` (LTO enabled in `Cargo.toml`); smoke-test the
   binary's `--help` and a `--dry-run`.
5. **GitHub Release** from the tag; attach the release binary (e.g.
   `telecli-x.y.z-<os>-<arch>`), the checksum, and the `CHANGELOG.md` section.
6. **crates.io only on explicit order** — this is a personal tool; publishing the
   crate is not part of the default path.

Rollback: no production servers to revert — point users at the previous tag;
`git tag` + GitHub Release can be deleted and re-cut if a release is broken.
SemVer PATCH on top is preferred over tag rewriting once a release is out.

## Dependency updates

- **Dependabot** (or `gitsum`) for Rust deps: weekly, one PR per update.
- grammers changes need care: after any `grammers-client` / `grammers-session`
  bump, diff the client methods + TL layer and update `docs/capabilities.md`
  (add rows; do not silently drop — matrix is the spine).
- Do not auto-merge grammers majors; treat them as a feature slice (matrix diff +
  live re-verification).

## Related

- ADR-005: [no release until want-matrix is done](decisions/005-unpublished-until-want-done.md)
- ADR-006: [Rust/grammers pivot](decisions/006-rust-grammers.md)
- Contract: `docs/cli-contract.md` (what counts as breaking)
