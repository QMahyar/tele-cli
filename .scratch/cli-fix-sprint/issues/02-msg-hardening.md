# 02 — msg command hardening: empty chat, --no-preview, react conflict

**What to build:** three validation fixes in the `msg` group:

1. **Empty chat targets rejected.** `--chat ""` currently passes validation (dry-run exits 0 with `"would":"send message to chat "`). Any `--chat`/`--from`/`--to` value that is empty or whitespace-only must be a Usage error (exit 1) before any connect. Apply to every msg subcommand carrying a chat target (send, edit, delete, forward from/to, pin, get, read, react, search, download) and to chat.rs commands that have validators (join, leave, participants, stats, admin-log, kick, invite, admin). Use one shared helper (e.g. `require_chat_target(value, flag)` in `src/commands/mod.rs`) so the rule is enforced everywhere, not copy-pasted.

2. **`--no-preview` becomes real.** Today `--preview` is `#[arg(long, default_value_t = true)]` with no negation: `--no-preview` is rejected by clap, and the check `if !args.preview { "--no-preview is not supported with --file" }` in `validate_send` is dead code. Add a `--no-preview` flag (clap `ArgAction::SetTrue`) that disables the link preview. Effective preview = `preview && !no_preview`. The `--no-preview` + `--file` rejection becomes reachable and must fire (exit 1). The dry-run payload must carry the effective preview value; the live `InputMessage` must use it too.

3. **`--reaction` and `--remove` are mutually exclusive.** `msg react --reaction 👍 --remove` currently exits 0 and silently ignores the reaction. Reject both together with a Usage error (exit 1). Add clap `conflicts_with` and/or enforce in `validate_react`.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

**Verified context (commit a647834):** `src/commands/msg.rs` — `SendArgs.preview` (line 43), `validate_send` (173-218, dead `--no-preview` check at 201-206), `validate_react` (949-956), `ReactArgs` (124-134); per-subcommand validators at 471/520/659/810. `src/commands/chat.rs` validators at 211/526/953.

**Acceptance criteria:**
- [ ] RED test first per fix (three failing tests before three implementations)
- [ ] `--chat ""` / whitespace-only rejected for all listed msg subcommands and chat validators (exit 1, UsageError envelope in --json)
- [ ] `--no-preview` accepted; `--no-preview --file` rejected with the existing message; dry-run payload shows preview:false; `--preview`/default still sends previews
- [ ] `react --reaction X --remove` rejected with a clear message, exit 1
- [ ] No comments added to code; project conventions followed (AGENTS.md)
- [ ] `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check` all pass
- [ ] Commit as `fix:` prefix, one logical change, on branch `fix/02-msg-hardening`