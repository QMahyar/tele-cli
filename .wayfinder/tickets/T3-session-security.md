# T3 — fix/session-security-hardening

Labels: wayfinder:task
Branch: fix/t3-session-security
Blocked by: T2

## Question

Session files, their sidecars, and Windows ACLs have four gaps between "restricted by intent" and "restricted in fact" — which close without breaking logout/relogin?

## Scope

1. session.rs:93-98 — SessionLock::drop closes then unlinks: window lets two processes both believe they hold the lock. Fix: do not delete lock files on release (tolerate stale zero-byte marker), OR delete only while still holding a verified lock. Pick the simpler provably-safe option; keep existing tests passing, add regression test.
2. session.rs:126-127 — restrict only `<name>.session`; SQLite creates `.session-journal`/`-wal`/`-shm` with default perms carrying auth-key material. After open, restrict every sibling matching `{name}.session*`. Close TOCTOU where feasible (pre-create restricted before SqliteSession::open if the API allows).
3. fs_util.rs:85-93 — SetNamedSecurityInfoW passes DACL_SECURITY_INFORMATION without PROTECTED_DACL_SECURITY_INFORMATION → inherited parent ACEs survive. Add the protected flag.
4. fs_util.rs:3-8 — create_dir_private has no Windows branch; apply explicit-DACL treatment to app-data dir + sessions subdir on Windows.
5. msg.rs:335-344 validate_upload_path — extend case-insensitive basename/suffix blocklist: id_rsa/id_ed25519 style keys, *.pem, *.key, *.p12, *.pfx, *.kdbx, .netrc, .git-credentials, aws/credentials pattern. Keep over-blocking bias; add tests for each new family incl. Windows trailing-dot/space and ADS bypass attempts already covered.
6. config.rs:330-340 write_config — tmp file created with default perms before restrict; reorder: create empty → restrict → write → rename.
7. config.rs:193 — `let _ = restrict_file_private(&path)` silently ignores failure to tighten an EXISTING .env; warn via logging when file exists but tightening failed.
8. config.rs:268,273 — config errors embed full path leaking username into stderr+JSON envelopes; reduce to leaf filename or generic location.

## Done when

Regression tests for each; logout/relogin flow unaffected per unit tests; clippy/fmt/tests green.
