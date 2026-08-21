# T6 — test/hardening-gaps

Status: closed
Labels: wayfinder:task
Branch: fix/t6-test-gaps
Blocked by: T1, T2, T3, T4, T5

## Question

The suite passes 640/640 but never exercises shutdown, adversarial input volume, or generated input — which gaps can close offline without network?

## Scope (proptest approved as dev-dependency)

1. Add `proptest` under `[dev-dependencies]`.
2. Property tests (offline):
   - `message_to_json`: for arbitrary text/media-name/chat-title strings, output always serializes and never panics; round-trip serde_json.
   - `.env` parser: parse→serialize round-trip losslessness on arbitrary key/value lines incl. malformed ones (skipped, not echoed).
   - `classify_target`: phone-branch-precedes-numeric invariant holds for arbitrary strings.
3. Shutdown-path test: drive listen stream loop's cancellation branch with a pre-fired signal; assert clean return within tokio timeout, no trailing row. Use existing seams only — no new public surface.
4. Oversized-input contract tests (contract.rs style): ~5 MB `--args` JSON and 100 KB `--chat` string → exit 1 + UsageError envelope, bounded wall time, no panic.

## Done when

All new tests pass offline in suite; no runtime deps added; fmt/clippy/tests green.

## Notes

If a property test exposes a real parser bug, fix it minimally in this ticket and record it in the resolution.
