# F4: integration sweep — deep-link targets, device echo, SLOWMODE seconds

**Branch:** `feat/f4-integration` · **Files:** `src/commands/msg.rs`, `src/commands/account.rs`, `src/error.rs` · **Deps:** none

## Goal

Wire three already-shipped primitives into their consumer surfaces.

## Acceptance

- [ ] Deep-link targets: when `--chat` parses via entities::parse_target to ResolvedTarget WITH Some(msg_id): `msg get` uses it as the target message when `--id` absent; `--id` + link-msg-id together = Usage conflict; other commands (send/edit/delete/react/vote/...) treat link-with-msg-id as Usage error naming which commands accept it (get today; list grows later). Tests: parse-through fixtures offline
- [ ] `tele account status` gains additive `device` object from config::account_identity (device_model/system_version/app_version/lang_code, nulls omitted) — proves W1-5 wiring user-visibly
- [ ] error envelope: SLOWMODE_WAIT now carries `seconds` exactly like FLOOD_WAIT (both are RPC 420; extend the existing wait_seconds special-case + its tests)
- [ ] Offline tests per item; gates green (fmt/clippy -D warnings/cargo test)

## Boundaries

Only `src/commands/msg.rs`, `src/commands/account.rs`, `src/error.rs`. serve.rs dispatch reuses cores — do not change core signatures (additive Params fields OK if needed, report it).
