# Live Testing Guide

> Online (live) tests exercise telecli against real Telegram user accounts.
> Default `cargo test` is 100% offline. Live tests are opt-in and gated.

---

## 1. Risk Model

A live test suite drives real Telegram USER accounts through the MTProto API. Every call
touches production Telegram infrastructure; mistakes can degrade or ban accounts.

### 1.1 Threat Table

| # | Threat | Severity | Likelihood | Mitigation |
|---|--------|----------|------------|------------|
| T1 | **FloodWait storm** — too many calls too fast triggers `FLOOD_WAIT_X` (server-side rate limit per method). grammers `AutoSleep` retries once if wait ≤ 60 s; longer waits propagate to the caller. | Low | Medium | Sequential default (`--parallel 1`). Respect `FLOOD_WAIT` / `SLOWMODE_WAIT`. No burst sends. |
| T2 | **PeerFlood / account restriction** — Telegram suspects spam when the ratio of messages to new contacts vs. existing dialogs is high. Duration is undefined; account is restricted to messaging existing contacts only. | High | Low (test suite stays read-only or sends to `me`) | Never cold-DM strangers from test accounts. Write tests target Saved Messages (`me`) only. |
| T3 | **SpamBot ban** — user-reported spam accumulates; 3–7 reports in 1–3 days can trigger temporary or permanent restriction. | High | Very Low | Test suite never messages strangers or posts to public groups. Only read-only ops or self-targeted writes. |
| T4 | **Mass join/leave flag** — joining > 10–20 groups/day, especially from a new account, triggers restrictions. | Medium | Low (Phase 3 only) | Phase 3 joins limited to explicit user go; cap at 1 join per 10 minutes; use disposable accounts. |
| T5 | **Transport flood (429)** — too many TCP connections from one IP in a short window. | Low | Low | Single connection per account; no parallel connection storms. grammers handles reconnection. |
| T6 | **Session invalidation** — user terminates all sessions from another client, or Telegram revokes the auth key (e.g., password change, account deletion, `AUTH_KEY_DUPLICATED` from concurrent access). | Medium | Low | Phase 0 preflight checks session freshness. Suite fails loudly on stale/missing sessions. |
| T7 | **Account frozen (userbot detection)** — patterns resembling automation (uniform timing, datacenter IP, identical device fingerprint across accounts) trigger `FrozenMethodInvalidError` / `FrozenParticipantMissingError`. | High | Very Low (2 accounts, human-like cadence) | Single session per account; no concurrent access; avoid datacenter proxies. |
| T8 | **SlowModeWait** — per-chat admin-set throttle on message sending. | Negligible | Low | Test suite never writes to third-party chats. If hit, respect the wait. |

### 1.2 Risk posture summary

Telecli's live test suite operates in a narrow, safe band: two logged-in accounts, sequential
operations, no cold outreach, no public group spam. The primary risks are T1 (FloodWait) and
T6 (session invalidation), both detectable and recoverable. T2/T3/T7 are unlikely given the
conservative operation profile but are the highest-severity if they occur.

---

## 2. Safe-Op Classification

Every telecli command is classified for live-test suitability. The `--parallel` flag must be
`1` for any live test run unless explicitly documented otherwise.

### 2.1 SAFE-ish — Low risk, suitable for automated live tests

| Command | Notes |
|---------|-------|
| `account status --json` | Read-only; required for preflight. |
| `account list` | Read-only; lists configured accounts. |
| `dialog list` | Read-only; iterates dialogs (cheap method per grammers docs). |
| `msg get` | Read-only; fetches messages by ID. |
| `msg search` | Read-only; search within a chat. |
| `msg download` | Read-only file download; use small test files only. |
| `msg send --to me` | Writes only to Saved Messages (self). Safe; but clean up test messages. |
| `takeout start` / `takeout export` / `takeout finish` | Write-once export; no side effects on chats. |

### 2.2 CAUTION — Moderate risk; requires cleanup or explicit user confirmation

| Command | Notes |
|---------|-------|
| `msg edit --to me` | Edit a self-sent message in Saved Messages. Clean up after. |
| `msg delete --to me` | Delete self-sent messages. Required cleanup step. |
| `msg react` | Reactions on own messages in Saved Messages. Remove after test. |
| `msg pin` / `msg read` | Pin/read on own messages. Low risk but verify undo. |
| `profile get` / `profile set` | Read is safe; write (name, bio, photo) requires cleanup. |

### 2.3 RISKY — High risk; must not run without explicit user go and disposable accounts

| Command | Notes |
|---------|-------|
| `msg send` (to others) | Cold DMs trigger PeerFlood quickly. |
| `msg forward` | Mass forwarding is a primary ban trigger. |
| `chat join` / `chat leave` | > 10–20 joins/day flags accounts. |
| `chat create` | Creates persistent groups/channels. |
| `contact add` | Adds numbers to contact list; mass import is abusable. |
| `msg send --parallel N` (fan-out) | Multi-account sends look like coordinated spam. |

### 2.4 Rate caps for live tests

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| `--parallel` | `1` (always for live) | ADR-004; prevents burst patterns. |
| Max sends per account per run | ≤ 5 (to `me` only) | Minimal self-message footprint. |
| Max joins per run | 0 (Phase 0–2); ≤ 1 (Phase 3) | Join velocity is a ban trigger. |
| Delay between writes | ≥ 5 s | Avoids FloodWait on sendMessage. |
| Max test duration | 5 min | Bounded blast radius. |

---

## 3. Phased Plan

### Phase 0 — Preflight (mandatory gate)

**Purpose:** Verify that the operator machine has the expected accounts, sessions are fresh,
and the suite can safely proceed. If preflight fails, the suite **fails loudly** (non-zero
exit, clear error message). It never silently re-authenticates.

**Steps:**

1. Run `tele account status --json`.
2. Parse the JSON output. Require that accounts named `1` and `2` both appear with
   `"authorized": true`.
3. If any expected account is missing, unauthorized, or the command fails (e.g., session
   file missing/corrupt), the suite **FAILS LOUDLY** with a message like:

   ```
   LIVE TEST PREFLIGHT FAILED
   Account '1': session missing or unauthorized
   Account '2': OK
   Expected accounts: ['1', '2']
   Recovery: run 'tele account login' for each missing account, then re-run the suite.
   ```

4. Skip any account entry named `me` that is not authorized (this is a known config
   artifact; it should not block the suite).
5. If preflight passes, the suite proceeds to Phase 1.

**Why fail loudly, not skip:**
Silent skips hide broken environments. An operator who sees "0 tests ran" may assume
everything passed. A loud failure with recovery instructions is unambiguous.

**Manual re-login recovery steps (documented in the failure message):**

```bash
# For each missing/unauthorized account:
tele account login          # interactive prompt for phone + code/QR
tele account status --json  # verify it shows authorized: true
```

If the session file exists but is stale (Telegram revoked it), delete and re-login:

```bash
# Windows
del %APPDATA%\telecli\sessions\<name>.session
# Linux/macOS
rm ~/.telecli/sessions/<name>.session

tele account login
```

### Phase 1 — Read-only operations

**What it tests:** `account status`, `dialog list`, `msg get`, `msg search`.

**What it needs that Phase 0 does not:** Nothing. Phase 0 confirms sessions are live;
Phase 1 only reads.

**No writes, no side effects.** This is the safest phase and can run any time.

### Phase 2 — Writes to Saved Messages (`me`) with cleanup

**What it tests:** `msg send --to me`, `msg edit`, `msg delete`, `msg react`, `msg pin`.

**What it needs that Phase 1 does not:**
- Write access to the Telegram API (sendMessage, editMessage, deleteMessages).
- A cleanup step that removes all test messages created during the run.
- The suite must track message IDs created during the run and delete them in a
  `finally`-style teardown block. If cleanup fails, the test must log which message IDs
  were left behind so the operator can remove them manually.

**Cleanup is mandatory.** Leaving test messages in Saved Messages is poor hygiene and
pollutes the operator's real Telegram environment.

### Phase 3 — Limited real operations (explicit user go required)

**What it tests:** `chat join`, `chat leave`, `msg send` to a designated test chat,
`contact add` (with a disposable number).

**What it needs that Phase 2 does not:**
- Explicit operator confirmation (the suite must block and prompt, or require a
  `TELE_LIVE_PHASE=3` env var to proceed past Phase 2).
- Disposable accounts: Phase 3 ops that touch third-party chats or contacts should
  run on throwaway accounts, not the operator's primary accounts.
- A designated test chat (pre-created, known channel/group) for send operations.
- Join/leave cleanup: leave any joined chats at the end of the run.

**Phase 3 is not implemented in the initial live-test harness.** It is documented here as
a future requirement. When implemented, it must:
- Refuse to run unless `TELE_LIVE_PHASE=3` is explicitly set.
- Use the disposable account path.
- Log every join/leave/send with timestamps for audit.

---

## 4. Harness Shape

### 4.1 Recommendation: Rust integration tests gated by env var

**Approach:** Add a `tests/live/` directory with Rust integration test files. These tests are
annotated `#[ignore]` by default and run only when `TELE_LIVE=1` is set:

```bash
cargo test --test live -- --ignored   # requires TELE_LIVE=1 in env
cargo test                             # default: runs ONLY offline tests
```

### 4.2 Justification

| Criterion | Rust `#[ignore]` tests | Shell script | Separate binary |
|-----------|----------------------|--------------|-----------------|
| `cargo test` stays offline | ✅ `#[ignore]` by default | ✅ separate script | ✅ separate binary |
| CI never runs live tests | ✅ `#[ignore]` + env gate | ✅ not in CI pipeline | ✅ not in CI |
| Shares telecli's dependencies | ✅ same Cargo workspace | ❌ shells out to `tele` | ✅ but duplicates deps |
| Type-safe result parsing | ✅ parses JSON natively | ❌ jq/text parsing | ✅ |
| Structured teardown | ✅ Rust RAII / Drop | ⚠️ trap-based, fragile | ✅ |
| Easy to add new test cases | ✅ `#[test]` functions | ⚠️ append to script | ✅ |
| Operator-facing output | ✅ `cargo test` harness | ✅ stdout | ✅ |

**Why not a shell script:** Shell scripts cannot natively parse `tele ... --json` output
safely, have fragile cleanup (traps can miss edge cases), and do not integrate with
`cargo test` reporting. They also cannot share the project's dependency graph.

**Why not a separate binary:** A separate binary duplicates the dependency tree and build
artifact. Integration tests in `tests/` are the idiomatic Rust approach for external
interface testing and integrate naturally with `cargo test`.

### 4.3 Directory layout

```
tests/
├── contract.rs            # existing offline tests
├── selection.rs           # existing offline tests
└── live/
    ├── mod.rs             # shared helpers (preflight, cleanup, account resolution)
    ├── preflight.rs       # Phase 0: account status gate
    ├── read_only.rs       # Phase 1: dialog list, msg get, msg search
    └── self_write.rs      # Phase 2: send/edit/delete to me with cleanup
```

### 4.4 Gate mechanism

The preflight module checks `std::env::var("TELE_LIVE")`. If absent or not `"1"`, all
live tests skip with a clear message:

```
test live::preflight::accounts_authorized ... ignored (set TELE_LIVE=1 to run)
```

Within the test functions, `#[ignore]` is the outer gate; `TELE_LIVE` is the inner gate.
This two-layer design means:
- `cargo test` never attempts to connect (no network calls, no session access).
- `cargo test -- --ignored` without `TELE_LIVE=1` skips cleanly.
- `TELE_LIVE=1 cargo test -- --ignored` runs the full live suite.

### 4.5 What CI does

CI runs `cargo test` (no `--ignored`). Live tests are never executed in CI. There is no
CI workflow for live tests and none is planned (see §6).

---

## 5. Session Freshness

### 5.1 What happens when sessions are gone or revoked

Telegram sessions can become invalid for several reasons:

| Cause | Symptom | Frequency |
|-------|---------|-----------|
| User terminated all sessions from another client | `grammers` returns `InvocationError::Rpc` with AUTH_KEY_UNREGISTERED or similar; grammers `AutoSleep` does not retry auth errors | Rare (operator-initiated) |
| Telegram revoked the auth key (security event) | Same as above; session file exists but is useless | Very rare |
| Session file missing or corrupt | `FileSession` cannot load; `connect()` fails at startup | Occasional (disk issues, accidental deletion) |
| Concurrent access (`AUTH_KEY_DUPLICATED`) | Two processes using the same session file; grammers may error or corrupt state | Avoidable; never share sessions |

### 5.2 How the harness detects stale sessions

Phase 0 (`tele account status --json`) is the detection mechanism. The command:
1. Opens the session file.
2. Attempts to call `users.getFullUser` (or equivalent) with the stored auth key.
3. Returns `{"name":"1","authorized":true,"me":{...}}` on success.
4. Returns `{"name":"1","authorized":false}` or an error on failure.

If `authorized` is `false` or the command exits with an error, the session is stale.
The harness treats this as a preflight failure (§3, Phase 0).

### 5.3 Recovery procedure

1. The harness prints which accounts failed and why.
2. The operator runs `tele account login` for each failed account (interactive; requires
   phone number and verification code or QR scan).
3. The operator re-runs the live suite.

The harness does **not** attempt automated re-login because:
- Login requires interactive verification (code from SMS/app, QR scan).
- Automated login would need to store 2FA passwords, which violates the security model
  (see `docs/security.md`: "2FA passwords are never accepted on argv; read from stdin only").
- Silent re-login could mask a compromised session.

---

## 6. Account Hygiene

### 6.1 Disposable accounts for Phase 3

Phase 3 operations (chat joins, sends to third-party chats, contact adds) should run on
disposable throwaway accounts, not the operator's primary Telegram accounts. Reasons:

- A ban on a primary account disrupts the operator's real communications.
- Disposable accounts can be created, warmed up, used for tests, and discarded.
- If a disposable account is banned, the loss is trivial.

For Phase 1–2 (read-only and self-writes), the operator's real accounts are acceptable
because the risk profile is minimal.

### 6.2 No CI runs

Live tests must **never** run from CI. Reasons:

- CI environments use datacenter IPs, which are a known risk factor for account
  restrictions (datacenter ASNs have elevated abuse priors in Telegram's scoring).
- CI cannot provide interactive login for session recovery.
- Secrets (TELE_API_ID, TELE_API_HASH) would need to be stored as CI secrets, which
  expands the attack surface for no benefit.

If a manually-triggered CI workflow is ever desired (NOT in this task), it would require:
- A GitHub Actions workflow with `workflow_dispatch` trigger.
- Secrets injected as environment variables.
- A runner with a residential/mobile IP (not a datacenter IP).
- An operator available to handle interactive login prompts.
- **None of this is being implemented now.**

### 6.3 Session file hygiene

- Session files live in `%APPDATA%\telecli\sessions/` (Windows) or `~/.telecli/sessions/`
  (Linux/macOS). Never in the repo.
- Never share a session file between processes or machines.
- Delete stale session files before re-login (don't leave orphaned auth keys).
- `tele account logout` revokes the session server-side; prefer it over manual file
  deletion when possible.

### 6.4 2FA considerations

- If a test account has 2FA enabled, the operator must enter the password manually
  during login. The harness never stores or prompts for 2FA passwords.
- Consider disabling 2FA on disposable test accounts for convenience, but be aware
  this reduces account security.

---

## 7. Scrubbed Data

This document contains no real phone numbers, API hashes, session strings, or 2FA
passwords. All examples use placeholder values. Do not commit real credentials.

---


## 8. Executed Live Verification — 2026-08-22 (v0.4.0 wave)

Real sessions 1 and 2, sequential cadence, self-targeted writes plus a private
test supergroup/channel created for the purpose. All checks passed unless noted.

| Surface | Result |
|---|---|
| account status/list (4 sessions; 2 authorized) | PASS |
| msg send/get/edit/delete roundtrip on Saved Messages | PASS |
| album send (3 files) via repeatable --file | PASS |
| listen --events Album cross-account coalescing (incl. catch-up replay) | PASS |
| msg send --copy-from/--copy-id (media re-send without forward header) | PASS |
| msg download --chunk-size-kb 64 streaming | PASS |
| msg search per-chat and --global | PASS |
| chat create (supergroup / forum / channel) | PASS |
| topic create/list/send --topic/pin/close/reopen (delete unexercised live) | PASS |
| chat settings read-back + slow-mode toggle; noforwards = honest layer error | PASS |
| chat edit title/about; invite export with options; join via link | PASS |
| participants + role filters; promote incl. manage_topics; kick --ban --duration | PASS |
| dialog draft set/clear/list; pin/unpin; delete left/cleared semantics | PASS |
| profile username set/restore; emoji-status remove (set needs a real owned emoji id -> DOCUMENT_INVALID surfaced honestly) | PASS |
| privacy get all 14 keys; allow/deny overlap rejection | PASS |
| msg read --mark-unread / --mentions | PASS |
| takeout progress lines, checkpointed export, finish --abandon | PASS |
| raw: channels.GetFullChannel, account.GetAuthorizations, messages.Search (top_msg_id), SetAuthorizationTTL gating + dry-run | PASS |

Environment limitations observed (not bugs): reactions in Saved Messages are
premium-gated (PREMIUM_ACCOUNT_REQUIRED); the layer has no noforwards toggle
method; owned-channel deletion is not a CLI capability.

Two bugs found by this pass were fixed before ship: the CAP-3 flag rename
(--files) that broke --file compatibility, and search --global still
requiring --chat.
## Sources

| Source | Type | Citation |
|--------|------|----------|
| Telegram API errors documentation | Official | https://core.telegram.org/api/errors |
| Telegram Bot API FAQ (rate limits) | Official | https://core.telegram.org/bots/faq |
| Telegram test environment / test DCs | Official | https://core.telegram.org/bug-bounty, https://core.telegram.org/api/auth |
| Telegram spam FAQ | Official | https://telegram.org/faq_spam |
| grammers `AutoSleep` docs | Official (library) | https://docs.rs/grammers-client/latest/grammers_client/client/struct.AutoSleep.html |
| grammers `ClientConfiguration` / retry policy | Official (library) | https://docs.rs/grammers-client/latest/grammers_client/client/struct.ClientConfiguration.html |
| grammers lib.rs (expensive vs cheap methods, peer flood) | Official (library) | https://docs.rs/grammers-client/latest/src/grammers_client/lib.rs.html |
| Telethon `flood_sleep_threshold` docs | Official (library) | https://docs.telethon.dev/en/stable/modules/client.html |
| Telethon RPC errors / FAQ | Official (library) | https://docs.telethon.dev/en/stable/concepts/errors.html |
| teleproto production docs (ban patterns, frozen accounts) | Community | https://docs.teleproto.dev/production |
| Telega rate limits guide 2026 | Community | https://telega.to/blog/telegram-rate-limits-for-automation-2026 |
| TG:ON reverse-engineering SpamBot signals | Community (reverse-engineered) | https://tg-on.com/articles/21-spambot-signals.html |
| TG:ON anti-ban arbitrage | Community | https://tg-on.com/articles/01-anti-ban-arbitrage.en.html |
| Entergram multi-account management guide | Community | https://www.entergram.com/blog/how-to-manage-10-telegram-accounts-without-getting-banned |
| ADR-004 (project decision) | Project | `docs/decisions/004-flood-and-parallel.md` |
| Security threat model | Project | `docs/security.md` |
