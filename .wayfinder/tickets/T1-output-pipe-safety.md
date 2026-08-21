# T1 — fix/output-pipe-safety

Status: closed
Labels: wayfinder:task
Branch: fix/t1-output-pipe-safety
Blocked by: (none — first)

## Question

Human-output paths panic on broken pipes while JSON paths propagate cleanly — can every stdout/stderr write survive `tele ... | head`?

## Scope

1. HIGH — output.rs:80 print_table and output.rs:100 print_account_table end in `.expect("write table to stdout")` → panic exit 101 on closed pipe. Make both return TeleResult<()>; callers in commands handle via existing error flow. Map ErrorKind::BrokenPipe to a silent clean exit 0 at the main boundary (Unix CLI convention) — decide the exact choke point (main.rs top-level match or a helper) and apply it uniformly.
2. Same class: raw println! loops at raw.rs:72, profile.rs:117, account.rs:107 — route through a shared fallible writer helper in output.rs.
3. stderr panics: output.rs:58 eprintln! and logging.rs:19 — same treatment, lower stakes; never let stderr failure abort a successful command.
4. comfy-table width: output.rs:88 Table::new() with ContentArrangement::Disabled overflows narrow terminals (60-char draft columns). Set ContentArrangement::Dynamic. Do NOT add terminal_size dependency unless already present — check Cargo.toml first; if absent, Dynamic arrangement alone is acceptable.
5. Tests: broken-pipe behavior is hard to unit test portably on Windows — cover the fallible-writer contract with a failing Write impl instead (assert no panic, error propagates), mirroring output.rs:271-276 closed-pipe JSON test.

## Done when

`cargo run -- account list | pwsh -c "$input | Select-Object -First 1"` style flows exit 0 (manual smoke optional); all writes go through fallible paths; clippy/fmt/tests green.

## Notes

Files owned by this ticket: src/output.rs, src/logging.rs, src/commands/raw.rs, src/commands/profile.rs, src/commands/account.rs (println sites + main boundary only), src/main.rs (boundary only).
