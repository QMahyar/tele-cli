# T2 — fix/auth-and-error-surfaces

Labels: wayfinder:task
Branch: fix/t2-auth-error-surfaces
Blocked by: T1

## Question

The account's strongest factor crosses a plaintext channel once, and machine consumers must regex error strings — what makes both surfaces honest without breaking the JSON contract?

## Scope

1. HIGH — 2FA password echo (account.rs:284-291): prompt_line uses plain read_line with terminal echo ON. Fix with no-echo read on Windows via windows crate SetConsoleMode (clear ENABLE_ECHO_INPUT; additive feature flags to existing dep are fine). On non-Windows, NO new dependency is approved: print a visible "password input will be echoed" warning instead and read normally. Never log the password either path.
2. HIGH — RPC code/name discarded (error.rs:104-110 invocation_error/as_json): additively emit `"code"` and `"name"` fields for InvocationError in as_json() (serde skip_serializing_if None; pure addition per cli-contract.md). Keep display string unchanged.
3. MEDIUM — Dropped translation only in one branch (entities.rs:79-108): translate grammers `InvocationError::Dropped` / "request error: dropped (cancelled)" ONCE inside invocation_message()/invocation_error() to actionable text ("peer unknown to this session; run tele dialog list to refresh cache"); remove/absorb the now-redundant special-case in the positive-id fallback branch.
4. LOW — serialize.rs:49-55 peer_key emits `id: 0` sentinel when bare id unavailable → omit key or emit null (key presence already varies; additive-safe).
5. LOW — serialize.rs:57-94 media_name conflates kind+label with colon join. Additive split: add `media_kind` + `media_label` fields alongside existing media string; keep old string byte-identical this release.
6. LOW — account.rs:488-496 argv_phone_warning only fires when stderr not a TTY; scope it to always warn when --phone passed interactively or not.
7. LOW — QR login raw URI printed to non-TTY stderr (account.rs:551-569 + client.rs:113-116): gate raw-URI fallback behind explicit `--show-token` opt-in flag (new clap flag on login subcommand); ASCII QR to stderr stays.

## Done when

New fields appear in --json envelopes with tests locking exact key sets; echo-off verified via unit-testable abstraction where possible; clippy/fmt/tests green.

## Notes

Contract rule: additive only. Existing envelope keys must not change shape. Update docs/cli-contract.md rows for new fields in THIS ticket (same-change rule).
