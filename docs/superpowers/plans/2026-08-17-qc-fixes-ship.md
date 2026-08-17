# QC Fixes -> Test -> Verify -> Ship Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix every actionable finding from the 2026-08-17 ten-agent QC run on Tele-Cli (v0.1.1), with a test per fix, full gate verification, and a v0.1.2 ship.

**Architecture:** Fifteen ordered tasks, each RED (failing test) -> GREEN (minimal fix) -> commit, in the repo's slice-workflow style. Contract/exit-code semantics first (machine API), then security, then dry-run/perf/UX, then docs/npm, then test coverage, then the versioned ship. No worktrees (user instruction) — work on `main` directly, one logical commit per task.

**Tech Stack:** Rust stable (edition 2021), grammers 0.10, clap 4, tokio, serde_json, comfy-table. No new runtime dependencies (Task 13 is a decision gate).

**Spec:** The 10 QC agent findings (QC1-QC10) summarized in the session; `file:line` citations re-verified against HEAD (9174b89). Docs: `docs/cli-contract.md` (machine API), `docs/capabilities.md` (matrix), `docs/release.md` (ship), `tasks/todo.md` (tracker).

## Global Constraints

- No code comments unless the user asks (AGENTS.md).
- No worktrees — commit directly on `main` (user instruction).
- Commit prefix `feat|fix|refactor|test|docs|chore:`, one logical change per commit.
- After EVERY task: `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `cargo test` green (offline — never Telegram from tests).
- JSON output changes additive-only (cli-contract.md); exit codes locked 0/1/2/3/4/130 (`src/error.rs:1-6`).
- No new runtime dependencies without approval (Task 13).
- `CHANGELOG.md` entry written with each change (release.md:42): each task adds one line under `[Unreleased]`; T15 renames it to `[0.1.2]`.
- Secrets never logged: api_hash, session, phone, 2FA, one-time login tokens.

## Finding -> Task Map

| QC finding | Task |
|---|---|
| Clap errors bypass `--json` envelope (QC2#1, QC10#2); `--json --jsonl` conflict lacks envelope | T1 |
| `msg forward` exits 0 when chunks fail (QC5) | T2 |
| `remove_session` deletes without lock (QC3 MED-2) | T3 |
| Upload blocklist incomplete, no size cap, download TOCTOU (QC3 MED-3/LOW-1/LOW-2) | T4 |
| Malformed phone -> exit 3 not 1; upload flood `seconds` dropped (QC5) | T5 |
| Dry-run payloads omit argument keys (QC4) | T6 |
| `raw` Debug dump in machine output (QC4) | T7 |
| takeout 1 spawn_blocking/row; listen unbounded channel + blocking write (QC6) | T8 |
| `--parallel` silent clamp; `adminlog` vs `admin-log` help (QC2#2/#3) | T9 |
| QR login URI printed to non-terminal stderr (QC10#1) | T10 |
| npm wrapper: wrong install command, 404 releases link (QC1) | T11 |
| completions/helpers/client test gaps; flaky test (QC7, QC9) | T12 |
| Windows permission no-op + temp-dir fallback (QC3 MED-1) — decision gate | T13 |
| Docs/matrix drift: test count, ideas.md "locked", todo.md contradiction, release.md contradiction, README "no npm", `completions` undocumented (QC1/QC4/QC8) | T14 |
| Verify gates + ship v0.1.2 (tag/release/publish need approval) | T15 |

## Deferred (documented, not fixed)

- Unbounded cumulative AutoSleep retry (QC5): grammers-internal (`src/client.rs:43-46`); cannot bound without wrapping every invoke. Note in `docs/security.md` (T14).
- Unicode-normalization path-guard bypass (QC3 LOW-3): defense would reject valid filenames. Note in `docs/security.md` (T14).
- Poisoned-mutex unwraps (`src/config.rs:191,212,238,244`): poison-only, not user-reachable (QC5/QC9 agree safe).
- listen permit held for stream lifetime (QC6 MED-3): correct bounded behavior; one doc line in cli-contract.md (T14).
- `authorize()` unit test (QC7): needs a Client with dead sender handle; not worth it for a 5-line wrapper.
- Binary name `telecli.exe` in `--help` (QC2#4): normal clap behavior; completions correct.

---

### Task 1: Pre-flight JSON envelope for clap errors

**Files:** `src/main.rs:107-152`, `tests/contract.rs` (~line 412), `docs/cli-contract.md` (~line 68), `CHANGELOG.md`

**Interfaces:** Consumes `output::Envelope::failed(dry_run: bool, command: &str, error: Value)` (`src/output.rs:36`) and `output::print_json` (`src/output.rs:61`). Produces `fn argv_command_hint() -> Option<String>` in `src/main.rs` — best-effort "msg send"-style path from argv.

- [ ] **Step 1: Failing tests** — append in `tests/contract.rs` after `json_and_jsonl_are_rejected` (line 417):

```rust
#[test]
fn clap_parse_error_emits_json_envelope() {
    let (code, out, err) = run_isolated("badsub", &["--json", "foobar"]);
    assert_eq!(code, 1, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["command"], serde_json::json!("foobar"));
    assert_eq!(v["results"], serde_json::json!([]));
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
    assert!(v["error"]["message"].as_str().unwrap_or_default().contains("foobar"), "stdout: {out}");
}

#[test]
fn missing_required_arg_emits_json_envelope() {
    let (code, out, err) = run_isolated("badarg", &["--json", "msg", "send"]);
    assert_eq!(code, 1, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["command"], serde_json::json!("msg send"));
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
}

#[test]
fn json_jsonl_conflict_emits_envelope() {
    let (code, out, err) = run_isolated("both2", &["account", "list", "--json", "--jsonl"]);
    assert_eq!(code, 1, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["ok"], serde_json::json!(false));
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
    assert!(v["error"]["message"].as_str().unwrap_or_default().contains("mutually exclusive"));
}
```

- [ ] **Step 2: Verify they fail** — `cargo test --test contract clap_parse_error missing_required_arg json_jsonl_conflict` -> 3 FAILs (stdout empty, `parse_json` panics).
- [ ] **Step 3: Implement** in `src/main.rs`.

Add next to `invoked_path` (line 158):

```rust
fn argv_command_hint() -> Option<String> {
    let mut parts = Vec::new();
    let mut skip_value = false;
    for arg in std::env::args_os().skip(1) {
        let s = arg.to_string_lossy().into_owned();
        if skip_value {
            skip_value = false;
            continue;
        }
        if matches!(s.as_str(), "--account" | "--tag" | "--parallel" | "--config") {
            skip_value = true;
            continue;
        }
        if s.starts_with("--account=")
            || s.starts_with("--tag=")
            || s.starts_with("--parallel=")
            || s.starts_with("--config=")
        {
            continue;
        }
        if s.starts_with('-') {
            continue;
        }
        parts.push(s);
        if parts.len() == 2 {
            break;
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}
```

Replace the `Err(e)` arm of `try_get_matches` (lines 111-119) with:

```rust
        Err(e) => {
            let code = if e.use_stderr() {
                error::EXIT_USAGE
            } else {
                error::EXIT_OK
            };
            let _ = e.print();
            if e.use_stderr() && std::env::args_os().any(|a| a == "--json" || a == "--jsonl") {
                let hint = argv_command_hint().unwrap_or_default();
                let error_json = serde_json::json!({"type": "UsageError", "message": e.to_string()});
                let envelope = output::Envelope::failed(false, &hint, error_json);
                if let Ok(v) = serde_json::to_value(&envelope) {
                    let _ = output::print_json(&v);
                }
            }
            std::process::exit(code);
        }
```

Replace the `--json/--jsonl` conflict block (lines 134-140) with:

```rust
    if flags.json && flags.jsonl {
        let message = "--json and --jsonl are mutually exclusive; pick one";
        output::log_line("error", message);
        let error_json = serde_json::json!({"type": "UsageError", "message": message});
        let envelope = output::Envelope::failed(false, &flags.command, error_json);
        if let Ok(v) = serde_json::to_value(&envelope) {
            let _ = output::print_json(&v);
        }
        std::process::exit(error::EXIT_USAGE);
    }
```

- [ ] **Step 4: Verify** — `cargo test --test contract`; clippy; fmt. All green.
- [ ] **Step 5: Docs + commit** — cli-contract.md pre-flight section: append "Clap parse errors (unknown subcommand, missing required flag) and the `--json`/`--jsonl` conflict also emit this envelope on stdout when `--json` or `--jsonl` is present." CHANGELOG `[Unreleased]`: `- Fixed: usage errors now emit the JSON error envelope on stdout in machine mode (was: empty stdout, exit 1).`

```bash
git add src/main.rs tests/contract.rs docs/cli-contract.md CHANGELOG.md
git commit -m "fix: emit JSON error envelope for clap-level usage errors"
```

---

### Task 2: `msg forward` marks partial when chunks fail

**Files:** `src/commands/msg.rs:721-736` (`forward_report`), tests module (near line 1696), `CHANGELOG.md`

**Interfaces:** Signature unchanged: `forward_report(requested, forwarded, dropped, failed) -> (Value, bool)` (second element stays `should_warn`). Produces `"partial": true` in the value when `requested > 0 && forwarded.len() < requested` — `envelope_exit_code` (`src/executor.rs:179-188`) already maps `data.partial` to EXIT_PARTIAL (same mechanism as `delete_report`, `msg.rs:502-509`).

- [ ] **Step 1: Failing tests** in `src/commands/msg.rs` tests module:

```rust
#[test]
fn forward_report_marks_partial_when_all_chunks_fail() {
    let (value, should_warn) = forward_report(3, &[], &[], &[1, 2, 3]);
    assert_eq!(value["partial"], serde_json::json!(true));
    assert_eq!(value["failed"]["count"], serde_json::json!(3));
    assert!(should_warn);
}

#[test]
fn forward_report_marks_partial_when_some_dropped() {
    let (value, _) = forward_report(2, &[serde_json::json!({"id": 1})], &[2], &[]);
    assert_eq!(value["partial"], serde_json::json!(true));
}

#[test]
fn forward_report_omits_partial_when_all_forwarded() {
    let (value, _) = forward_report(1, &[serde_json::json!({"id": 1})], &[], &[]);
    assert!(value.get("partial").is_none(), "value: {value}");
}
```

- [ ] **Step 2: Verify they fail** — `cargo test forward_report` -> FAIL, `partial` key absent.
- [ ] **Step 3: Implement** — replace body of `forward_report` (lines 727-735):

```rust
    let mut value = serde_json::json!({"requested": requested, "forwarded": forwarded});
    if !dropped.is_empty() {
        value["dropped"] = serde_json::json!({"count": dropped.len(), "ids": dropped});
    }
    if !failed.is_empty() {
        value["failed"] = serde_json::json!({"count": failed.len(), "ids": failed});
    }
    let partial = requested > 0 && forwarded.len() < requested;
    if partial {
        value["partial"] = serde_json::json!(true);
    }
    let should_warn = requested > 0 && forwarded.is_empty();
    (value, should_warn)
```

Keep existing tests `forward_report_tracks_dropped_and_failed` (1696) and `forward_report_warns_when_nothing_confirmed` (1709) passing; extend the latter to also assert `partial == true`.
- [ ] **Step 4: Verify** — `cargo test msg::tests`; clippy; fmt.
- [ ] **Step 5: Changelog + commit** — `- Fixed: msg forward now exits 2 (partial) when some or all chunks fail; was exit 0.`

```bash
git add src/commands/msg.rs CHANGELOG.md
git commit -m "fix: msg forward reports partial when chunks fail"
```

---

### Task 3: `remove_session` respects the session lock

**Files:** `src/session.rs:49-59`, tests module (near line 157), `CHANGELOG.md`

**Interfaces:** Consumes `lock_path(name)` (`src/session.rs:30`) and `File::try_lock` (same mechanism as `open_session`, line 77). Produces unchanged `remove_session(name) -> anyhow::Result<()>` — now errors "session {name} is in use by another process" when the lock is held.

- [ ] **Step 1: Failing test** in `src/session.rs` (copy `TELE_APP_DIR` setup from `open_session_rejects_concurrent_use`, line 140):

```rust
#[tokio::test]
async fn remove_session_rejects_in_use_session() {
    let _guard = lock_env().await;
    let dir = test_dir("session-lock-remove-held");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("TELE_APP_DIR", &dir);
    let held = open_session("work").await.unwrap();
    let err = remove_session("work").unwrap_err();
    assert!(err.to_string().contains("is in use by another process"));
    drop(held);
    remove_session("work").unwrap();
    assert!(!session_path("work").exists());
    assert!(!lock_path("work").exists());
    std::env::remove_var("TELE_APP_DIR");
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Verify it fails** — `cargo test session::tests::remove_session_rejects_in_use_session` -> FAIL (deletes while `held` alive).
- [ ] **Step 3: Implement** — replace `remove_session` (lines 49-59):

```rust
pub fn remove_session(name: &str) -> anyhow::Result<()> {
    validate_name(name).map_err(anyhow::Error::msg)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path(name))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(anyhow::anyhow!("session {name} is in use by another process"));
        }
        Err(e) => return Err(e.into()),
    }
    drop(lock);
    for path in [session_path(name), lock_path(name)] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
```

`drop(lock)` before `remove_file` matters on Windows (open files cannot be deleted).
- [ ] **Step 4: Verify** — `cargo test session::tests` (incl. existing `remove_session_cleans_session_and_lock_files`); clippy; fmt.
- [ ] **Step 5: Changelog + commit** — `- Fixed: account remove refuses to delete a session file in use by another process.`

```bash
git add src/session.rs CHANGELOG.md
git commit -m "fix: remove_session respects the session lock"
```

---

### Task 4: Upload/download guard hardening

**Files:** `src/commands/msg.rs:309-329` (`validate_upload_path`), `src/commands/msg.rs:1136-1141` (download `create_dir_all`), tests module (~line 2058), `CHANGELOG.md`

**Interfaces:** Produces `const MAX_UPLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;` and `fn check_upload_size(bytes: u64) -> TeleResult<()>` in `src/commands/msg.rs`.

- [ ] **Step 1: Failing tests** in `src/commands/msg.rs` tests module (reuse existing temp-dir helper):

```rust
#[test]
fn check_upload_size_accepts_boundary() {
    assert!(check_upload_size(MAX_UPLOAD_BYTES).is_ok());
}

#[test]
fn check_upload_size_rejects_over_cap() {
    let err = check_upload_size(MAX_UPLOAD_BYTES + 1).unwrap_err();
    assert!(matches!(err, TeleError::Usage(_)));
    assert!(err.message().contains("2 GiB"));
}

#[test]
fn validate_upload_path_rejects_config_toml_basename() {
    let dir = temp_path("uploadcfg");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["config.toml", "config.toml.tmp-123", "CONFIG.TOML"] {
        let path = dir.join(name);
        std::fs::write(&path, b"x").unwrap();
        let err = validate_upload_path(path.to_str().unwrap()).unwrap_err();
        assert!(matches!(err, TeleError::Usage(_)), "{name}: {err:?}");
        assert!(err.message().contains("sensitive"), "{name}");
    }
    let ok_path = dir.join("notes.txt");
    std::fs::write(&ok_path, b"x").unwrap();
    validate_upload_path(ok_path.to_str().unwrap()).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}
```

- [ ] **Step 2: Verify they fail** — `cargo test check_upload_size validate_upload_path_rejects_config` -> FAIL (not found / not rejected).
- [ ] **Step 3: Implement**

(a) Add next to `validate_upload_path`:

```rust
const MAX_UPLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

fn check_upload_size(bytes: u64) -> TeleResult<()> {
    if bytes > MAX_UPLOAD_BYTES {
        return Err(TeleError::Usage(format!(
            "refusing to upload file larger than 2 GiB (got {bytes} bytes)"
        )));
    }
    Ok(())
}
```

(b) In `validate_upload_path` (lines 319-327) replace the blocklist and tail:

```rust
    let lower = base.to_ascii_lowercase();
    if lower == ".env"
        || lower.ends_with(".session")
        || lower.ends_with(".session-journal")
        || lower == "config.toml"
        || lower.starts_with("config.toml.")
    {
        return Err(TeleError::Usage(format!(
            "refusing to upload sensitive file {base}"
        )));
    }
    let path = std::path::Path::new(path);
    if !path.is_file() {
        return Err(TeleError::Usage(format!("upload file not found: {path:?}")));
    }
    check_upload_size(std::fs::metadata(path)?.len())
```

(`std::io::Error` -> `TeleError` via existing `From` at `src/error.rs:67`.)

(c) TOCTOU re-check — after the download `create_dir_all` block (lines 1136-1141) and its `??`, insert:

```rust
            validate_download_dir(&out_dir)?;
```

`out_dir` now exists, so `resolve_for_guard` canonicalizes the full path and a junction planted in the missing tail is visible to `path_under_guard`. (No offline RED here: junction creation needs Windows admin; existing download-guard tests at `msg.rs:2058+` stay green; this is a defensive second check.)
- [ ] **Step 4: Verify** — `cargo test msg::tests`; clippy; fmt.
- [ ] **Step 5: Changelog + commit** — `- Fixed: uploads of config.toml are refused; uploads over 2 GiB are refused; download dirs re-checked after creation (junction TOCTOU).`

```bash
git add src/commands/msg.rs CHANGELOG.md
git commit -m "fix: harden upload blocklist, size cap, and download dir re-check"
```

---

### Task 5: Peer-target error classification + upload flood seconds

**Files:** `src/entities.rs:7-16,89`, all `resolve_peer` call sites (`grep -rn "resolve_peer(" src/` — msg.rs, chat.rs, listen.rs), `src/commands/msg.rs:396-401`, `tests/contract.rs`, `CHANGELOG.md`

**Interfaces:** Produces `pub async fn resolve_peer(client, session, target) -> TeleResult<grammers_client::peer::Peer>` (was `Result<_, InvocationError>`); `Target::Phone("")` -> `TeleError::Usage("invalid phone target: use +<digits>")` (exit 1).

- [ ] **Step 1: Failing tests** — in `tests/contract.rs`:

```rust
#[test]
fn malformed_phone_target_exits_usage() {
    let (code, out, err) = run_isolated("badphone", &["--json", "msg", "send", "--chat", "+", "--text", "hi"]);
    assert_eq!(code, 1, "stderr: {err}");
    let v = parse_json(&out);
    assert_eq!(v["error"]["type"], serde_json::json!("UsageError"));
    assert!(v["error"]["message"].as_str().unwrap_or_default().contains("phone"), "stdout: {out}");
}
```

In `src/entities.rs` tests module (classify tests exist at line ~600+):

```rust
#[test]
fn plus_only_and_non_digit_phones_classify_as_phone_with_no_digits() {
    assert_eq!(classify_target("+"), Target::Phone(String::new()));
    assert_eq!(classify_target("+abc"), Target::Phone(String::new()));
}
```

- [ ] **Step 2: Verify they fail** — contract test FAILs with exit 3 (not 1).
- [ ] **Step 3: Implement**

(a) `src/entities.rs`: change `resolve_peer` signature (line 11) to `-> crate::error::TeleResult<grammers_client::peer::Peer>`; phone branch (lines 13-16) becomes:

```rust
        Target::Phone(digits) => {
            if digits.is_empty() {
                return Err(crate::error::TeleError::Usage(
                    "invalid phone target: use +<digits>".to_string(),
                ));
            }
```

Wrap every `client.invoke(...)?` / `client.resolve_peer(...).await?` in the body with `map_err(crate::error::invocation_error)`; the two synthetic `rpc_error(400, ...)` returns become `Err(crate::error::invocation_error(rpc_error(...)))`. Compiler pinpoints each.

(b) Call sites: `entities::resolve_peer(...).await.map_err(tele_invocation)?` -> `entities::resolve_peer(...).await?` (all inside `TeleResult` fns). In `src/commands/listen.rs:117-137` the Err arm becomes `handle_stream_failure(&name, e, &mut failures, deadline).await?; continue;` (drop the `TeleError::Other(format!("cannot resolve chat ..."))` wrapper). Loop `cargo check` until clean.

(c) `src/commands/msg.rs:401` upload error: replace `.map_err(|e| TeleError::Other(e.to_string()))?` with `.map_err(tele_invocation)?`; if the compiler shows `upload_file` returns a non-`InvocationError` type, use instead:

```rust
                    .map_err(|e| match e {
                        grammers_client::InvocationError::Rpc(rpc) if rpc.code == 420 => {
                            TeleError::Invocation(rpc.to_string(), rpc.value)
                        }
                        other => TeleError::Other(other.to_string()),
                    })?;
```

Goal: 420 floods from uploads carry `seconds` (contract cli-contract.md:56-66; mechanism `error.rs:92-99`).
- [ ] **Step 4: Verify** — full `cargo test` (entities suite locks the taxonomy); clippy; fmt.
- [ ] **Step 5: Changelog + commit** — `- Fixed: malformed phone chat targets are usage errors (exit 1); upload flood waits carry seconds in the JSON error.`

```bash
git add src/entities.rs src/commands/msg.rs src/commands/chat.rs src/commands/listen.rs tests/contract.rs CHANGELOG.md
git commit -m "fix: classify malformed phone targets as usage errors, keep upload flood seconds"
```

---

### Task 6: Dry-run payloads carry the command's argument keys

**Files:** `src/commands/msg.rs:383-387` (send), `src/commands/msg.rs:454-458` (edit), `src/commands/account.rs:146` (add), `src/commands/chat.rs:736-740` (stats), `src/commands/takeout.rs:66-73` (start), `src/commands/raw.rs:51-55` (raw), their in-file test modules, `CHANGELOG.md`

**Interfaces:** Arg structs — `SendArgs` (msg.rs:31: chat, text, schedule, file, caption, reply, preview, format, silent), `EditArgs` (msg.rs:56: chat, id, text), `AddArgs` (account.rs:20: name, tags), `StatsArgs` (chat.rs:113: chat, broadcast), `StartArgs` (takeout.rs:25: contacts, messages, photos), `RawArgs` (raw.rs:12: name, args). Produces dry-run `data` with every arg key (null when unset); `would`/`dry_run` stay — additive only.

- [ ] **Step 1: Failing tests** — extend each module's existing dry-run test. Representative (msg.rs):

```rust
#[test]
fn send_dry_run_carries_argument_keys() {
    let value = serde_json::json!({
        "dry_run": true, "chat": "@x", "text": "hi", "file": serde_json::Value::Null,
        "caption": serde_json::Value::Null, "format": "plain", "schedule": serde_json::Value::Null,
        "reply": serde_json::Value::Null, "preview": true, "silent": false,
        "would": "send message to chat @x"
    });
    assert_eq!(value["text"], serde_json::json!("hi"));
    assert_eq!(value["format"], serde_json::json!("plain"));
    assert_eq!(value["preview"], serde_json::json!(true));
}
```

Mirror for the other five: `name`+`tags` (add), `chat`+`text` (edit), `broadcast` (stats), `contacts`+`messages`+`photos` (start), `args` (raw). If an existing test locks the exact payload, update it instead of duplicating.
- [ ] **Step 2: Verify they fail** — `cargo test dry_run` -> FAILs.
- [ ] **Step 3: Implement** — add the keys to each dry-run object:
  (a) send: `"text": text, "file": file, "caption": caption, "format": format, "schedule": schedule, "reply": reply, "preview": preview, "silent": silent` (values already cloned in the closure).
  (b) edit: add `"chat": chat_target, "text": text`.
  (c) account add (line 146): replace `&dry_run_envelope(&args.name, &would, &flags.command)` with:

```rust
            &action_envelope(
                &args.name,
                serde_json::json!({"would": would, "dry_run": true, "name": args.name, "tags": args.tags}),
                true,
                &flags.command,
            ),
```

  (d) stats: add `"broadcast": broadcast`.
  (e) start: add `"contacts": contacts, "messages": messages, "photos": photos`.
  (f) raw: add `"args": params` (already-parsed `serde_json::Value`).
- [ ] **Step 4: Verify** — `cargo test`; clippy; fmt.
- [ ] **Step 5: Changelog + commit** — `- Fixed: --dry-run payloads now include the command's own argument keys (additive JSON).`

```bash
git add src/commands/msg.rs src/commands/account.rs src/commands/chat.rs src/commands/takeout.rs src/commands/raw.rs CHANGELOG.md
git commit -m "fix: dry-run payloads carry the command's argument keys"
```

---

### Task 7: `raw` GetAllDrafts — no Debug dump in machine output

**Files:** `src/commands/raw.rs:316-339`, tests module (16 existing tests), `CHANGELOG.md`

**Interfaces:** Consumes `tl::enums::Updates` variants `Updates`/`Combined`/`NotModified` (reference: takeout.rs:340-360) and existing `update_summary` (raw.rs:331). Produces pure `fn all_drafts_summary(r: &tl::enums::Updates) -> serde_json::Value`.

- [ ] **Step 1: Failing test** in `src/commands/raw.rs` tests module:

```rust
#[test]
fn all_drafts_summary_never_emits_debug_strings() {
    let not_modified = tl::enums::Updates::NotModified(tl::types::UpdatesNotModified { date: 0 });
    let v = all_drafts_summary(&not_modified);
    assert_eq!(v["kind"], serde_json::json!("NotModified"));
    assert!(!v.to_string().contains("UpdatesNotModified"));

    let combined = tl::enums::Updates::Combined(tl::types::UpdatesCombined {
        updates: Vec::new(), users: Vec::new(), chats: Vec::new(),
        date: 0, seq_start: 0, seq: 0,
    });
    let v = all_drafts_summary(&combined);
    assert_eq!(v["kind"], serde_json::json!("Combined"));
    assert!(!v.to_string().contains("UpdatesCombined"));
}
```

(Adjust field names to grammers 0.10 generated types if the compiler disagrees — takeout.rs Combined arm is the reference.)
- [ ] **Step 2: Verify it fails** — `cargo test all_drafts_summary` -> FAIL (fn not defined).
- [ ] **Step 3: Implement** — extract the GetAllDrafts arm body into `all_drafts_summary`:

```rust
fn all_drafts_summary(r: &tl::enums::Updates) -> serde_json::Value {
    match r {
        tl::enums::Updates::Updates(u) => serde_json::json!({
            "updates": u.updates.iter().map(update_summary).collect::<Vec<_>>(),
            "users": u.users.len(),
            "chats": u.chats.len(),
        }),
        tl::enums::Updates::Combined(c) => serde_json::json!({
            "updates": c.updates.iter().map(update_summary).collect::<Vec<_>>(),
            "users": c.users.len(),
            "chats": c.chats.len(),
            "kind": "Combined",
        }),
        tl::enums::Updates::NotModified(_) => serde_json::json!({
            "updates": [],
            "users": 0,
            "chats": 0,
            "kind": "NotModified",
        }),
    }
}
```

Call it from the GetAllDrafts arm (lines 320-339): `Ok(all_drafts_summary(&r))`.
- [ ] **Step 4: Verify** — `cargo test raw::tests`; clippy; fmt.
- [ ] **Step 5: Changelog + commit** — `- Fixed: raw messages.GetAllDrafts never dumps Debug strings into JSON output.`

```bash
git add src/commands/raw.rs CHANGELOG.md
git commit -m "fix: raw GetAllDrafts summary avoids Debug dumps in machine output"
```

---

### Task 8: takeout write batching + listen stdout backpressure

**Files:** `src/commands/takeout.rs:366-380`, `src/commands/listen.rs:231,251,268,283` (+ new `emit_row` helper), `CHANGELOG.md`

**Interfaces:** Consumes existing `raw_message_to_json`/`serde_json::to_string` in takeout loop and `event_row`/`raw_row` in listen. Produces `fn emit_row(value: serde_json::Value) -> TeleResult<()>` in `src/commands/listen.rs` (spawn_blocking write, awaited = backpressure).

- [ ] **Step 1: RED not practical offline** (both sites are network-bound loops). GREEN-only task; the existing 60 listen tests + 12 takeout tests must stay green. Implement and verify.
- [ ] **Step 2: Implement (a) takeout** — replace the per-message loop (lines 366-380):

```rust
                let mut lines = Vec::new();
                for raw in &msgs {
                    if count >= limit {
                        break;
                    }
                    let row = raw_message_to_json(raw, &m_peers, Some(chat_id))?;
                    lines.push(serde_json::to_string(&row)?);
                    count += 1;
                }
                if !lines.is_empty() {
                    messages_file = tokio::task::spawn_blocking(move || {
                        let mut file = messages_file;
                        for line in &lines {
                            writeln!(file, "{line}")?;
                        }
                        Ok::<_, std::io::Error>(file)
                    })
                    .await
                    .map_err(|e| TeleError::Other(e.to_string()))??;
                }
```

One spawn_blocking per page (<=100 rows) instead of per row.
- [ ] **Step 3: Implement (b) listen** — add helper near the top of the stream loop section:

```rust
fn emit_row(value: serde_json::Value) -> TeleResult<()> {
    let line = serde_json::to_string(&value)?;
    tokio::task::spawn_blocking(move || {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{line}")?;
        out.flush()
    })
    .await
    .map_err(|e| TeleError::Other(e.to_string()))??;
    Ok(())
}
```

Replace the four `output::print_json_result(&event_row(...))?` / `&raw_row(...)?` calls (listen.rs:231,251,268,283) with `emit_row(...)?`. Awaiting each spawn_blocking gives backpressure: a stalled stdout consumer stalls the update loop instead of growing the unbounded grammers channel.
- [ ] **Step 4: Verify** — `cargo test listen::tests takeout::tests`; clippy; fmt. Manual smoke (optional, live): `tele listen --events NewMessage --timeout 5` still streams JSONL.
- [ ] **Step 5: Changelog + commit** — `- Fixed: takeout export writes one batch per page; listen stdout writes are backpressured.`

```bash
git add src/commands/takeout.rs src/commands/listen.rs CHANGELOG.md
git commit -m "fix: batch takeout writes and backpressure listen stdout"
```

---

### Task 9: CLI UX minors

**Files:** `src/main.rs:77` (chat about text), `src/main.rs` after flags build (~line 133), `tests/contract.rs`, `CHANGELOG.md`

- [ ] **Step 1: Failing tests** in `tests/contract.rs`:

```rust
#[test]
fn parallel_out_of_range_warns_on_stderr() {
    let (code, _out, err) = run_isolated("parclamp", &["--parallel", "99", "account", "list"]);
    assert_eq!(code, 0, "stderr: {err}");
    assert!(err.contains("clamped"), "stderr: {err}");
}

#[test]
fn chat_help_mentions_admin_log_not_adminlog() {
    let (code, _out, err) = run_isolated("adminloghelp", &["chat", "adminlog", "--json"]);
    assert_eq!(code, 1, "stderr: {err}");
    assert!(err.contains("admin-log"), "stderr: {err}");
}
```

- [ ] **Step 2: Verify they fail** — `cargo test --test contract parallel_out_of_range chat_help_mentions` -> FAIL (no warning; no mention).
- [ ] **Step 3: Implement** — (a) main.rs:77: `"Chats: join, leave, invite, participants, kick, admin, adminlog, stats, create"` -> `"... kick, admin, admin-log, stats, create"`. (b) after `logging::set_flags(...)` (line 133) add:

```rust
    if let Some(p) = cli.parallel {
        if !(1..=3).contains(&p) {
            output::log_line("warn", &format!("--parallel {p} is outside 1-3; clamped"));
        }
    }
```

- [ ] **Step 4: Verify** — `cargo test --test contract`; clippy; fmt.
- [ ] **Step 5: Changelog + commit** — `- Fixed: help text says admin-log; out-of-range --parallel now warns.`

```bash
git add src/main.rs tests/contract.rs CHANGELOG.md
git commit -m "fix: correct chat help subcommand name, warn on parallel clamp"
```

---

### Task 10: QR login token on non-terminal stderr

**Files:** `src/commands/account.rs:478-490` (`render_qr`), `docs/observability.md` ("Never" list, line 32), `CHANGELOG.md`

**Interfaces:** Consumes `std::io::IsTerminal` (Rust std, no dep).

- [ ] **Step 1: RED not practical offline** (QR render path needs a login flow). GREEN + doc.
- [ ] **Step 2: Implement** — in `render_qr`, in the branch that prints the raw `tg://login?token=...` URI (the fallback when bitmap rendering fails, around line 490), before printing:

```rust
        if !std::io::stderr().is_terminal() {
            output::log_line(
                "warn",
                "printing one-time login token to a non-terminal stderr; treat the output as a secret",
            );
        }
```

- [ ] **Step 3: Docs** — `docs/observability.md` line 32 "Never:" list: append `, one-time login tokens (QR URI)`.
- [ ] **Step 4: Verify** — `cargo test account::tests`; clippy; fmt.
- [ ] **Step 5: Changelog + commit** — `- Fixed: QR login fallback warns when the one-time token is printed to a non-terminal.`

```bash
git add src/commands/account.rs docs/observability.md CHANGELOG.md
git commit -m "fix: warn when QR login token is printed to non-terminal stderr"
```

---

### Task 11: npm wrapper fixes

**Files:** `npm/bin/telecli.js:12`, `npm/README.md:14`, `CHANGELOG.md`

- [ ] **Step 1: Implement (a)** — `npm/bin/telecli.js` line 12: `"npm install -g telecli"` -> `"npm install -g @qmahyar/telecli"`.
- [ ] **Step 2: Implement (b)** — `npm/README.md` lines 14-15: remove the GitHub Releases link (repo is private; anonymous 404s — release.md:69). Replace with:

```md
Other platforms / manual install: `cargo install --locked telecli` (requires Rust stable).
```

- [ ] **Step 3: Verify** — `node --check npm/bin/telecli.js`; no test suite for npm; visual review of README.
- [ ] **Step 4: Changelog + commit** — `- Fixed: npm wrapper error message and README reference the correct scoped package.`

```bash
git add npm/bin/telecli.js npm/README.md CHANGELOG.md
git commit -m "fix: npm wrapper install instructions and README link"
```

---

### Task 12: Test coverage additions + flaky-test investigation

**Files:** `src/commands/completions.rs` (tests module), `src/commands/helpers.rs` (tests module), `src/client.rs` (tests module), `CHANGELOG.md`

- [ ] **Step 1: Failing tests** — (a) completions (markers are standard clap_complete output):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn gen(shell: clap_complete::Shell) -> String {
        let mut cmd = crate::command_for_completions();
        let mut buf = Vec::new();
        clap_complete::generate(shell, &mut cmd, "tele", &mut buf);
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn bash_completions_reference_tele() {
        assert!(gen(clap_complete::Shell::Bash).contains("complete -F _tele tele"));
    }

    #[test]
    fn zsh_completions_have_compdef() {
        assert!(gen(clap_complete::Shell::Zsh).contains("#compdef tele"));
    }

    #[test]
    fn fish_completions_have_complete() {
        assert!(gen(clap_complete::Shell::Fish).contains("complete -c tele"));
    }

    #[test]
    fn powershell_completions_register() {
        assert!(gen(clap_complete::Shell::PowerShell).contains("Register-ArgumentCompleter"));
    }
}
```

(b) helpers (TL type constructors per `src/commands/helpers.rs:11-33`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_period_json_shape() {
        let v = tl::enums::StatsDateRangeDays::Days(tl::types::StatsDateRangeDays {
            min_date: 100, max_date: 200,
        });
        assert_eq!(stats_period(&v), serde_json::json!({"min_date": 100, "max_date": 200}));
    }

    #[test]
    fn stats_abs_json_shape() {
        let v = tl::enums::StatsAbsValueAndPrev::Prev(tl::types::StatsAbsValueAndPrev {
            current: 12.5, previous: 10.0,
        });
        assert_eq!(stats_abs(&v), serde_json::json!({"current": 12.5, "previous": 10.0}));
    }

    #[test]
    fn stats_percent_json_shape() {
        let v = tl::enums::StatsPercentValue::Value(tl::types::StatsPercentValue {
            part: 50.0, total: 100.0,
        });
        assert_eq!(stats_percent(&v), serde_json::json!({"part": 50.0, "total": 100.0}));
    }
}
```

(c) client base64 (pure fn at `src/client.rs:142`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_url_encode_is_padless_url_safe() {
        assert_eq!(base64_url_encode(b"\xfb\xff"), "-_8");
        assert_eq!(base64_url_encode(b"f"), "Zg");
    }
}
```

- [ ] **Step 2: Verify they fail** — `cargo test completions::tests helpers::tests client::tests` -> FAIL (test modules/fns not found).
- [ ] **Step 3: Add the test modules** as written above (they compile against existing code — this task is pure test addition; the module attribute in (c) is `#[cfg(test)]` inside `src/client.rs`).
- [ ] **Step 4: Verify they pass + flaky investigation** — `cargo test` (full). Then run the bin-target suite 5 times to chase the QC9 one-off flake:

```bash
for i in 1..=5 { cargo test --bin telecli 2>&1 | Select-String -Pattern "FAILED|failed|passed" }
```

If a failure reproduces, capture the test name and fix the stdout-capture race in that test (likely a listen/output test asserting on captured stdout); if not reproduced in 5 runs, record "not reproduced — no action".
- [ ] **Step 5: Changelog + commit** — `- Test: completions output, chat stats JSON shape, QR base64 encoding (net +9 tests).`

```bash
git add src/commands/completions.rs src/commands/helpers.rs src/client.rs CHANGELOG.md
git commit -m "test: cover completions output, stats JSON shape, base64 encoding"
```

---

### Task 13: Windows app-dir fallback — decision gate

**Files:** `docs/security.md`, `CHANGELOG.md` (default option), or `src/config.rs:46` (option B)

**Context:** QC3 MED-1: `restrict_file_private`/`create_dir_private` are no-ops on Windows; `app_data_dir()` falls back to `std::env::temp_dir()` when APPDATA/HOME are missing. Reality check: `%APPDATA%` and `%TEMP%` are both user-private by default on Windows, and on Unix the dir/file perms ARE enforced (`fs_util.rs`), so the residual risk is shared machines with weakened default ACLs.

- [ ] **Step 1: Ask the user** which option:
  - **A (default, no dependency):** document in `docs/security.md`: Windows relies on the user-profile ACLs of `%APPDATA%`/`%TEMP%`; harden with a restricted DACL if the machine's default ACLs are weak. No code change.
  - **B (needs approval — adds `windows-sys` runtime dep):** apply an explicit restricted DACL to the app dir, `.env`, and session files on Windows.
- [ ] **Step 2 (option A):** append the note to `docs/security.md`; CHANGELOG `- Docs: Windows permission model documented (relies on user-profile ACLs).`
- [ ] **Step 3: Commit** — `docs: document Windows permission model`

```bash
git add docs/security.md CHANGELOG.md
git commit -m "docs: document Windows permission model"
```

---

### Task 14: Docs/matrix sync

**Files:** `AGENTS.md:20`, `README.md:21,185`, `docs/ideas/tele-cli.md:1-5,39-44,48-84,93`, `tasks/todo.md:106,130`, `docs/release.md:16,23`, `docs/capabilities.md` (kernel section), `docs/cli-contract.md` (completions + listen + pre-flight + dry-run), `CHANGELOG.md`

Run `cargo test` first and use the ACTUAL counts (expected ~556 after T12) everywhere.

- [ ] **Step 1: Test counts** — `AGENTS.md:20` ("528 tests"), `README.md:185`, `tasks/todo.md:130` -> actual count.
- [ ] **Step 2: ideas.md** — line 1-5 status: replace "Status: Spec (locked), v1.0" with "Status: superseded — see `docs/capabilities.md` (matrix) and ADR-006 (Rust/grammers pivot)"; the `want` rows (39-84) and proxy line 93 ("SOCKS and MTProto" -> "socks5-only") get a header note "rows below are pre-pivot; the live matrix is docs/capabilities.md".
- [ ] **Step 3: todo.md** — line 106 "ConfigError keeps exit 3" -> "ConfigError exits 1 (EXIT_USAGE)" to match line 120 and `error.rs:21`; update line 130's "528 tests green" count.
- [ ] **Step 4: release.md** — lines 16+23 contradiction: reword line 16 to "SemVer. Version is derived from git tags; `Cargo.toml` `version` is bumped in the release commit to match the tag (`0.1.2` for `v0.1.2`)."
- [ ] **Step 5: README.md:21** — replace "Unpublished for now — no npm package..." with "Install: `cargo build --release` (binary `target/release/telecli.exe`); an npm wrapper `@qmahyar/telecli` (win32-x64) is published per `docs/release.md` when you say so. Release gate (ADR-005) is met."
- [ ] **Step 6: capabilities.md** — add row `kernel.completions` -> `done` (command exists, `src/commands/completions.rs`).
- [ ] **Step 7: cli-contract.md** — add a `completions` command section (subcommands bash/zsh/fish/powershell, stdout output, exit 0); in the listen section add "listen always streams JSONL on stdout; `--json` is accepted as a no-op for symmetry"; in the dry-run section confirm argument keys (T6 made impl match the existing text).
- [ ] **Step 8: CHANGELOG + commit** — `- Docs: matrix, contract, and README synced with implementation (test counts, completions, listen JSONL).`

```bash
git add AGENTS.md README.md docs/ideas/tele-cli.md tasks/todo.md docs/release.md docs/capabilities.md docs/cli-contract.md CHANGELOG.md
git commit -m "docs: sync matrix, contract, and README with implementation"
```

---

### Task 15: Verify gates + ship v0.1.2

**Files:** `Cargo.toml` (version), `CHANGELOG.md`, npm wrapper already at 0.1.2 (`npm/package.json:3`)

**Requires explicit user approval for: tag push, GitHub Release, npm publish** (AGENTS.md ask-first; release.md:58 "Publish (when you explicitly say so)").

- [ ] **Step 1: Gates** — run in order; all must be clean:
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test` (full; record final count)
  - `cargo build --release` (no warnings)
  - smoke: `target\release\telecli.exe --help` and `target\release\telecli.exe account list --json --dry-run`
- [ ] **Step 2: Version + changelog** — `Cargo.toml` version `0.1.1` -> `0.1.2` (PATCH: fixes only, JSON changes additive and contract-conforming, exit codes unchanged). Rename `[Unreleased]` to `[0.1.2] - <today>`; the dated CHANGELOG entries from T1-T14 sit under it. Add a `Fixed` summary line if the per-task lines need grouping.
- [ ] **Step 3: Commit** — `chore: prepare v0.1.2 release` (includes Cargo.toml, Cargo.lock if touched, CHANGELOG.md).
- [ ] **Step 4: Tag (APPROVAL REQUIRED)** — `git tag -a -m "v0.1.2" v0.1.2` and push tags (release.md:63).
- [ ] **Step 5: GitHub Release (APPROVAL REQUIRED)** — from the tag: `target/release/telecli.exe` as `telecli-0.1.2-win32-x64.exe` + checksum + CHANGELOG section (release.md:66-67). Private repo — do not link anonymously.
- [ ] **Step 6: npm publish (APPROVAL REQUIRED)** — from `npm/`: `npm publish --access=public`; verify `npm install -g @qmahyar/telecli` then `telecli --version` (release.md:70-76). No install scripts in the package (bin/telecli.js spawns the bundled exe).
- [ ] **Step 7: Live checklist (optional, needs real sessions)** — the two gaps from `tasks/todo.md` manual list: `tele chat participants` on a group and `tele chat adminlog` on an admin channel; proxy via tor 9050 (`kernel.proxy` note, todo.md:62).
- [ ] **Step 8: todo.md** — mark this plan's tasks resolved with commit refs.

---

## Self-Review (run at plan end)

- **Spec coverage:** all 10 QC reports mapped — see Finding->Task table; QC1 all items in T11/T14/T15; QC2 in T1/T9; QC3 in T3/T4/T13 (+2 deferred); QC4 in T6/T7/T14; QC5 in T2/T5 (+1 deferred); QC6 in T8 (+2 doc-only); QC7 in T12/T14; QC8 in T14; QC9 in T12; QC10 in T1/T10.
- **Placeholder scan:** every code step carries real code or an exact edit; the only intentional non-determinism is T5(b) (compiler-enumerated call sites) and T8/T10/T13 (no offline RED — stated explicitly with the reason).
- **Type consistency:** `argv_command_hint() -> Option<String>` used by `Envelope::failed(false, &hint, ...)` (command: &str — borrow ok); `forward_report` signature unchanged; `resolve_peer -> TeleResult<Peer>` consumed with `?` at all sites; `check_upload_size(u64) -> TeleResult<()>`; `all_drafts_summary(&tl::enums::Updates) -> serde_json::Value`; `emit_row(Value) -> TeleResult<()>`.