# W1-5: kernel.device-id — per-account device identity

**Branch:** `feat/w1-5-device-id` · **Files:** `src/config.rs`, `src/client.rs` · **Deps:** none

## Goal

Each account presents its own device_model/system_version/app_version/lang_code to Telegram (multi-account hygiene).

## Acceptance

- [ ] Config: optional `[accounts.<name>]` keys `device_model`, `system_version`, `app_version`, `lang_code` (strings); unknown-key tolerance preserved (toml_edit carry-over test exists — extend, don't break)
- [ ] Sensible defaults when absent: current behavior unchanged (whatever ClientBuilder defaults are today) — zero regression for existing configs
- [ ] client.rs: feed values into SenderPool/Client builder params (`ConnectionParams` or builder equivalents — inspect grammers 0.10 fields; if a key has no grammers surface, skip it and note that in the row comment... NO comments — note it in your final report instead)
- [ ] `tele account status` gains additive `device` object echoing configured identity (proves wiring offline)
- [ ] Offline tests: TOML parse roundtrip per key, precedence (per-account over absent), status echo; gates green

## Boundaries

Only `src/config.rs` + `src/client.rs`. account.rs is owned by W1-4 concurrently — do NOT touch it; put the status echo wherever config/client exposure allows without editing account.rs (if impossible, report back instead of crossing the boundary).
