# Implementation Plan: Review Remediation Sweep (2026-08-26)

## Overview
Fix every finding from the 10-reviewer audit of 2026-08-26: 3 critical, 10 major,
6 security, 5 correctness, 4 optimization, plus documentation drift. Work is
partitioned by FILE OWNERSHIP so waves of parallel sub-agents cannot conflict.
TDD everywhere an offline test can exist (RED first). Full `cargo fmt && cargo
clippy -D warnings && cargo test` gates between waves.

## Waves (file-partitioned, parallel inside each wave)

### Wave 1 — Critical
- [ ] W1a mcp.rs: `--read-only`/`--groups` enforced in call_core; visible-count
      in unknown-tool error; op timeouts enforced on tool calls
- [ ] W1b listen.rs: per-event error containment (no `?` past reconnect loop);
      EPIPE = clean exit; dedupe keyed on update pts not state pts;
      GapTracker cap; log swallowed sync_update_state errors; drop dead
      `_parallel` binding
- [ ] W1c session.rs: failed import must never delete pre-existing session;
      export defaults out of CWD; sweep stale `.tmp-{pid}`; reject Windows
      reserved device names; guard symlinked export destination
- [ ] W1d serve.rs: fail undelivered jobs with id-correlated envelopes on
      StreamDown/resync; move dispatch off biased select loop; cache route
      table once; log sync_update_state failures

### Checkpoint 1
- [ ] fmt + clippy -D warnings + full test suite green

### Wave 2 — Major (messaging/chats/entities)
- [ ] W2a msg.rs + commands/mod.rs: album caption on first item only; reject or
      honor --schedule/--silent/--media-ttl/--background on url/album/copy
      paths; copy-from plain keeps caption; download dir guard before mkdir;
      unique .part temp names; delete --all reports unconfirmed; reject
      --global with --chat; fix vacuous files-key test; validate_limit rejects 0
- [ ] W2b entities.rs + dialog.rs + contact.rs: draft ids use bot-API channel
      form; purge temp contact when phone resolution finds no user; contact add
      honors one-sided name flags; unblock usage text says unblock; evict stale
      peer-cache entries + CHAT_ID_INVALID matcher; classify empty t.me/ as
      Usage; guard dialog delete against Saved Messages; server-side folder
      filter for dialog list
- [ ] W2c chat.rs + topic.rs: join rewrites bare invite hash to t.me link;
      paginate links/importers/requests past 100; usage-limit i32 range check;
      admin-log fetches until --since; stats/admin run ensure_chat_peer; reject
      --forum without supergroup; chat_id 0 becomes error; topic pin/unpin both
      paths; topic emoji rejected honestly instead of fake ok
- [ ] W2d takeout.rs + completions.rs: start refuses active takeout session;
      CHECKPOINT_DONE also on empty-page break; completions derive real bin
      name (telecli); EPIPE-safe stdout

### Checkpoint 2
- [ ] fmt + clippy -D warnings + full test suite green

### Wave 3 — Security + Correctness
- [ ] W3a account.rs: non-Windows echo-off via termios (or loud per-attempt
      warning); redact phone in prompts; purge pending/{name}.*.json on
      logout/remove/success; logout holds lock while deleting; try_exists Err
      treated as unknown (no cleanup); SignedIn never deletes session; DC id
      out-of-range errors; InvalidPassword distinct error; SRP p/g checks on new
      password path; complete login bootstrap on ResendCode/QR success; raise
      post-signout deletion retry budget; refuse interactive ops under --parallel>1
- [ ] W3b rate_limiter.rs + error.rs + executor.rs + output.rs + logging.rs +
      fs_util.rs: Notified registered before token load; fractional refill kept;
      needs_wait cleared; BrokenPipe exit consistent; missing outcome exit_code
      defaults to EXIT_ALL_FAILED; Level enum for log_line; unified casing +
      debug tier reachable; UNC prefix rewrite; aligned TOKEN_USER read
- [ ] W3c privacy.rs + profile.rs: --allow/-deny union semantics (or documented
      replace + diff warning) — decision: UNION, matching incremental UX;
      namespace-tagged overlap keys; base-rule users resolved with real access
      hashes; username sentinel escape (--clear-username equivalent); bio limit
      70 + ABOUT_TOO_LONG mapped; UTF-16 length checks; photo mime/size
      validation; upload_file errors no longer TaskPanic; single limiter acquire

### Checkpoint 3
- [ ] fmt + clippy -D warnings + full test suite green

### Wave 4 — Optimizations + Cargo
- [ ] Cargo.toml: release strip + codegen-units=1; drop unused rand; drop dup
      dev serde_json; comment libsql pin rationale in docs/release.md note
- [ ] (serve route table, dialog folder filter, MCP timeouts already in W1/W2)

### Wave 5 — Documentation
- [ ] AGENTS.md: project map adds stickers/stories/mcp/serve; privacy 14 keys;
      account/chat/msg/topic/dialog/listen rows updated; test count refreshed;
      exit codes include 0/130
- [ ] README.md: raw registry count 25; command table gains sticker/story/
      serve/mcp
- [ ] docs/cli-contract.md: listen event kinds list completed; any behavior-
      changing fixes noted (privacy union semantics, bio 70, completions bin)
- [ ] tasks/todo.md: stale shipped checkboxes closed

## Risks and Mitigations
| Risk | Impact | Mitigation |
|------|--------|------------|
| Parallel agents collide | build breakage | strict file ownership per wave |
| Behavior change breaks contract tests | CI red | agents run targeted tests; I gate between waves |
| Network-path fixes unverifiable offline | silent regressions | unit-test pure logic; live behavior flagged for manual verify |

## Verification (final)
- [ ] cargo fmt --all -- --check
- [ ] cargo clippy --all-targets -- -D warnings
- [ ] cargo test (all green, count reported)
- [ ] docs consistent with shipped behavior
