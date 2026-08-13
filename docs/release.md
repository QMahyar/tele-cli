# Release and publish

Unpublished until `want` rows in `docs/capabilities.md` are `done` (except MCP/skill, which stay `later` until Phase 6). This document is the contract so we do not invent a second process later.

## Versioning

SemVer. Version is **derived from git tags**, not hand-edited in many files.

- `MAJOR` — exit codes, `--json` keys removed/renamed, removed commands
- `MINOR` — new commands or additive JSON keys
- `PATCH` — fixes, no contract change

`pyproject.toml` version should match the tag (`1.4.0` for `v1.4.0`). Prefer a single source (hatch-vcs or bump in the release commit).

## Git

- Trunk-based. `main` always has default pytest green.
- Branches: `feature/*`, `fix/*`, `chore/*`. Short-lived.
- Commits: `feat|fix|refactor|test|docs|chore:` — one logical change.
- Never force-push `main`. Never commit `.env` or `*.session`.

## Changelog

`CHANGELOG.md` is curated (Added / Changed / Fixed / Deprecated / Removed / Security). Write the entry **with the change**, not at release archaeology time.

## CI (when the repo is on GitHub)

On PR and push to `main`:

1. `uv sync --frozen`
2. `ruff check` + `ruff format --check`
3. `pytest` (default; **not** `-m live`)
4. No Telegram secrets in CI

Live tests run only on a developer machine with local sessions.

## Publish (when you explicitly say so)

1. Matrix: no remaining `want` (or you explicitly waive named ids).
2. `CHANGELOG.md` has the version section.
3. Tag `vX.Y.Z` annotated.
4. Build sdist+wheel with `uv build`.
5. Upload to PyPI only on your order (`uv publish` or trusted publishing).
6. GitHub Release from the tag; attach artifacts.

Rollback: yank the PyPI release if needed; point users at the previous tag. No production servers to revert.

## Dependabot

Weekly `uv` / GitHub Actions updates. Telethon minors: open PR + matrix diff task. Do not auto-merge Telethon majors.
