# 13 — listen exits 1 on config/credential failure (matches other commands)

**What to build:** `tele listen` with missing/broken credentials must exit 1 (usage) like every other command, not 3. `aggregate_exit` must honor the usage exit code pushed per account when the failure is a config/credential class.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: aggregate_exit with a usage outcome returns EXIT_USAGE (1), auth still 4, runtime still 3
- [ ] Offline test: listen creds-failure path produces exit 1
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md finding 1.3 residual (listen.rs aggregate_exit collapses to EXIT_ALL_FAILED)