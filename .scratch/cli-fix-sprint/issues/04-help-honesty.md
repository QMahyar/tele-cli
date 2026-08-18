# 04 — Docs/help honesty: --parallel range, --jsonl semantics, ANSI-free JSON errors

**What to build:** three "surface honesty" fixes so documented behavior matches actual behavior.

1. **`--parallel` range is really 1–32.** `AGENTS.md` and `executor::effective_parallel` (clamp 1..=32) say 1–32; the help text, the clamp warning, and `docs/cli-contract.md` say 1–3. Align all text to 1–32: clap help string, the `main.rs` warning (`--parallel {p} is outside 1-32; clamped`), and `docs/cli-contract.md`. Clamping behavior itself stays (values are clamped with a warning, not rejected) — the docs just tell the truth.

2. **`--jsonl` semantics documented.** One-shot commands emit a single envelope line under `--jsonl` (identical to `--json`); only `listen` emits one record per event. This is valid JSONL and intentional — but undocumented. Add a note to `docs/cli-contract.md` and clarify the `--jsonl` help text ("machine output: JSON lines (one-shot commands emit a single envelope line)").

3. **No ANSI escapes in JSON error messages.** When clap fails with `--json`/`--jsonl`, the envelope's `error.message` embeds clap's styled text including ANSI escape sequences (`\x1b[...m`), which is hostile to machine consumers. Strip ANSI escapes from the message before embedding it in the envelope (a small `strip_ansi` helper — regex-free, scan for `\x1b[` … `m`). Human stderr output may keep its styling. Unit-test the helper; also assert the envelope message contains no `\x1b`.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

**Verified context (commit a647834):** `src/main.rs` — parallel help (line 44), clamp warning (143-147), clap error envelope (`e.to_string()`, 117-126); `src/executor.rs:124` `effective_parallel` clamps 1..=32; `docs/cli-contract.md` lines ~12 (`--parallel N default 1; max 3`) and the `--jsonl` sections.

**Acceptance criteria:**
- [ ] RED test first for `strip_ansi` (fails on raw ANSI, passes on stripped)
- [ ] `--help` shows 1-32; `--parallel 99` warns "outside 1-32; clamped" on stderr
- [ ] `--json` + unknown flag → `error.message` contains no `\x1b` byte
- [ ] `docs/cli-contract.md` updated: parallel max 32, jsonl one-shot note
- [ ] `docs/capabilities.md` checked for parallel/jsonl references and updated if needed
- [ ] `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check` pass
- [ ] Commit as `fix:` or `docs:` prefix, one logical change, on branch `fix/04-help-honesty`