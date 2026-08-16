# 03 — takeout export wraps every request in InvokeWithTakeout

**What to build:** `tele takeout export` must wrap **all** data requests in the active takeout session (`tl::functions::InvokeWithTakeout` with the persisted `takeout_id`), not just dialogs and history. Today `GetContacts` runs unwrapped — the export is not uniformly rate-limit-relaxed. The takeout session must be genuinely used end-to-end.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: the contacts export path builds an `InvokeWithTakeout` wrapping `contacts.GetContacts` with the persisted takeout_id
- [ ] Offline test: takeout state read/write round-trips (id persisted in `export/<name>/takeout.json`, read back on export)
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 12.1 (GetContacts unwrapped at takeout.rs:222-226; dialogs/history already wrapped)