# F3: account password --remove via grammers-crypto

**Branch:** `feat/f3-password-remove` · **Files:** `src/commands/account.rs`, `Cargo.toml` · **Deps:** F2 merged (account.rs free)

## Goal

Unlock `tele account password --remove` using the now-approved direct `grammers-crypto` dependency (W1-4 verdict: `grammers_crypto::two_factor_auth::calculate_2fa` + `check_p_and_g` are public; client just does not re-export them).

## Acceptance

- [ ] Add `grammers-crypto = "0.10"` (match grammers-client's exact minor from Cargo.lock)
- [ ] --remove: prompt current password no-echo → GetPassword → build InputCheckPasswordSRP via calculate_2fa → UpdatePasswordSettings with empty new params (verify exact TL shape for removal at this layer: current_input_check_password with no new_settings hash) → confirm row
- [ ] --set/--change REMAIN honestly blocked (PH2 pbkdf2 private upstream) — keep existing blocker errors naming the gap; do not hand-roll
- [ ] Hint/recovery-email edits ride the same SRP proof where the layer allows without PH2 — attempt only if GetPassword shape permits; otherwise leave blocked and say so in report
- [ ] Secrets never logged/printed; offline tests: mode matrix unchanged, remove-path SRP construction unit-testable? (SRP needs server salt fixture — construct deterministic fixture from GetPassword shape; if impossible offline, test everything up to the RPC boundary and mark the boundary)
- [ ] Gates green

## Boundaries

Only `src/commands/account.rs` + Cargo.toml dep line.
