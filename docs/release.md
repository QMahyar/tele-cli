# Publish a release

A release is an annotated tag. You bump versions and push the tag. The `release` workflow in `.github/workflows/release.yml` builds the binaries, publishes the GitHub Release, and publishes npm. crates.io waits for an explicit order.

## Check the gates

1. Confirm `main` is green. The `ci` workflow (`.github/workflows/ci.yml`) gates on `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, an MSRV check on Rust 1.89, and `cargo audit`.
2. Confirm that `docs/capabilities.md` has no remaining `want` rows. The publication gate lives in [ADR-005](decisions/005-unpublished-until-want-done.md) and is currently met. Waive nothing silently.
3. Pick the version number. MAJOR breaks the contract, MINOR adds commands or JSON keys, PATCH fixes. The breaking-change definition sits in `docs/cli-contract.md` under "Stability". For why this stack exists, see [ADR-006](decisions/006-rust-grammers.md).

## Bump the version

1. Set `version` in `Cargo.toml` to the tag number without the `v` prefix. Use `0.6.2` for `v0.6.2`. Commit `Cargo.lock` in sync.
2. Add a `## [X.Y.Z] - YYYY-MM-DD` section to `CHANGELOG.md`. The workflow extracts the section whose heading matches the tag version, so the heading must match exactly. Write entries when you land changes, not at release time.
3. Commit with a `chore:`, `feat:`, or `fix:` prefix and push to `main`.

Never commit `.env`, `*.session`, phones, or api hashes.

## Tag the release

```bash
git tag -a -m "vX.Y.Z" vX.Y.Z
git push origin vX.Y.Z
```

The push triggers the `release` workflow.

## What the workflow builds

**Build job.** A matrix of 7 targets: windows x64, linux x64 (gnu + musl), linux arm64 (gnu + musl), macOS arm64, and macOS x64 cross-compiled on the arm64 runner. Each target produces `telecli-<version>-<os>-<arch>[.exe]` and a matching `.sha256` checksum file. Regenerate the count with `rg -c "target: " .github/workflows/release.yml`, which prints `7`.

**Release job.** Extracts the matching section from `CHANGELOG.md` as the release body and creates the GitHub Release through `softprops/action-gh-release@v2`. The finished release carries 14 assets: 7 binaries plus their 7 checksum files.

**npm job.** Publishes 8 packages through OIDC trusted publishing (no token). Seven platform packages (`@qmahyar/telecli-<os>-<arch>`) each bundle one binary with `os`/`cpu` guards; the main `@qmahyar/telecli` package ships only the JS launcher and pins every platform package under `optionalDependencies`, so `npm install -g @qmahyar/telecli` downloads just the matching binary. The `linux-arm64-musl` binary is static and runs in Termux/Android.

## Verify the release

1. Open the GitHub Release page and confirm 14 assets.
2. Download a binary and smoke-test it. `telecli --help` and `telecli --dry-run` must respond.
3. Run `npm view @qmahyar/telecli version` and confirm it shows the tag version. Then install and check:

```bash
npm install -g @qmahyar/telecli && telecli --version
```

## Handle an npm failure

Publishing authenticates through npm trusted publishing (OIDC). The `@qmahyar/telecli` package on npmjs.com must list this repository and the `release` workflow as its trusted publisher. OIDC tokens are short lived, so there is no secret to set or rotate. When publish fails, fix the trusted-publisher configuration on npmjs.com, then rerun the failed job:

```bash
gh run rerun <run-id> --job <npm-job-id>
```

The GitHub Release ships even when npm fails. To publish npm by hand instead, use your own npm login with publish rights:

```bash
cd npm && npm version <ver> --no-git-tag-version --allow-same-version && npm publish --access=public
```

## Order crates.io separately

crates.io publication is not part of this pipeline. It happens only when you explicitly order it, because this is a personal tool. Run `cargo publish` when ordered.

## Roll back

There are no servers to revert. Point users at the previous tag. While nobody depends on the broken release, delete the tag and the GitHub Release, then cut them again. Once a release is out, prefer a SemVer PATCH on top.
