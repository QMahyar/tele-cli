# Release and publish

Rust/grammers reality. Unpublished until `want` rows in `docs/capabilities.md` are
`done` — that gate is currently **met** (all rows `done`/`later`/`never`; MCP/skill
stay `later` until Phase 6), per [ADR-005](decisions/005-unpublished-until-want-done.md).
This document is the contract so we do not invent a second process later.

Open release-readiness work:

- CI exists (`ci` workflow in `.github/workflows/ci.yml`); plan below is the contract it implements.
- Trunk is `main` (renamed from `master`); the `ci` workflow triggers on both until stale refs clear.
- `v0.1.1` tag exists; `CHANGELOG.md` has the dated `[0.1.1] - 2026-08-17` section (and the `[0.1.0] - 2026-08-13` initial section).

## Versioning

SemVer. Version is derived from git tags; `Cargo.toml` `version` is bumped in the release commit to match the tag (`0.1.2` for `v0.1.2`).

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
- `Cargo.lock` is committed (it is a binary crate); keep it in sync in the
  release commit.

## Changelog

`CHANGELOG.md` is curated (Added / Changed / Fixed / Deprecated / Removed /
Security). **Write the entry with the change**, not at release archaeology time —
a change without its entry is unfinished. It is consumer-facing: describe CLI
behavior, not `git log`.

## CI

The `ci` workflow runs on PR and push to `main` (and `master` until stale refs
clear), on `ubuntu-latest` and `windows-latest`. In a fresh checkout:

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test` (default — **no network**, no Telegram; `tests/contract.rs` reads
   only local files)
4. No Telegram secrets in CI: never load `%APPDATA%/telecli/.env`, never create
   sessions. Live checks stay manual on a developer machine with real sessions.

## Release workflow (automated)

The `release` workflow (`.github/workflows/release.yml`) triggers on `v*` tag
pushes and automates building, GitHub Release creation, and (optionally) npm
publish.

### What CI does on tag push

1. **Build job** — matrix of 4 targets (windows x64, linux x64, macOS arm64,
   macOS x64 cross-compiled). Produces:
   - `telecli-<ver>-<os>-<arch>[.exe]` binary per target
   - Matching `.sha256` checksum file per binary
2. **Release job** — downloads all build artifacts, extracts the matching section
   from `CHANGELOG.md` as the release body, and creates/updates the GitHub Release
   via `softprops/action-gh-release@v2`.
3. **npm job** (conditional) — guarded by the `NPM_TOKEN` secret. Downloads the
   win-x64 binary, copies it into `npm/bin/telecli.exe`, bumps `npm/package.json`
   version to match the tag, and runs `npm publish --access=public` from the `npm/`
   directory. The npm tarball bundles the binary directly; it does **not** download
   release assets at install time (the repo is private and release assets 404
   anonymously).

### What stays manual

Before pushing the tag, **you** must:

1. **Version bump commit** — update `Cargo.toml` `version` to match the intended
   tag, add the `CHANGELOG.md` entry for the new version, and commit
   (`chore:` or `feat:`/`fix:` prefix as appropriate).
2. **Annotated tag push** — `git tag -a -m "vX.Y.Z" vX.Y.Z && git push origin
   vX.Y.Z`. This triggers the release workflow.
3. **NPM_TOKEN secret** — must be configured in the repository's Settings → Secrets
   and variables → Actions. Without it the npm job is skipped silently; GitHub
   Releases and binaries are still published.
4. **Verify** — check the GitHub Release page and (if npm was published) install
   with `npm install -g @qmahyar/telecli` and run `telecli --version`.

### crates.io — explicit order only

Publishing the crate to crates.io is **not** part of the automated pipeline and
must only happen on explicit instruction. This is a personal tool; crate
publication is a separate decision.

## Publish checklist (manual summary)

1. Matrix gate: no remaining `want` rows (currently met) — or you explicitly waive
   named ids.
2. `CHANGELOG.md` has a version section for the tag; `Cargo.toml` version matches.
3. Tag `vX.Y.Z` **annotated** (`git tag -a -m "vX.Y.Z"`) and push tags.
4. CI builds, creates GitHub Release, and (if NPM_TOKEN is set) publishes npm.
5. Smoke-test: verify binary `--help` and `--dry-run` from the release assets or
   from a local build.
6. **crates.io only on explicit order** (see above).
7. **npm** — publish is automated by CI when NPM_TOKEN is set. If you need to
   publish manually: `cd npm && npm version <ver> --no-git-tag-version && npm
   publish --access=public`.

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
