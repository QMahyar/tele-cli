# Implementation Plan: Remaining Code Review Fixes

## Mission
Implement all remaining fixes from Issues #3, #4, and #5, creating 3 branches with PRs ready to merge.

## Context
- **Completed:** Branch 1 (Critical Bugs) - PR #6 (pagination fix)
- **Remaining:** 26 fixes across 3 branches
- **Issues:** #3 (Security), #4 (UX), #5 (Low Priority)
- **Base:** main branch

## Branch 2: Security (Issue #3) - 6 Fixes

### H1: Windows File Permissions (fs_util.rs:15-17)
Add `windows` crate, implement proper ACL restrictions for .session and .env files

### M5: Mutex Poisoning (config.rs:195,242 + rate_limiter.rs:99)
Replace `.unwrap()` with `.unwrap_or_else(|e| e.into_inner())`

### M6: Windows Atomic Rename (config.rs:316-327)
Handle existing files on Windows before rename

### M8: Phone Redaction (account.rs:297-302)
Implement `redact_phone()` to show `+1***456` format

### M1: Config Double-Loading (executor.rs:30,125)
Load config once, pass to selection function

### M3: Redundant Creds (credentials.rs:7-9)
Load credentials once per fanout

## Branch 3: UX (Issue #4) - 5 Fixes

### H2: Admin Rights Granularity (chat.rs:482-519)
Add `--preset` and `--rights` flags for granular admin permissions

### M7: Emoji Validation (msg.rs:902-915)
Add `unicode-segmentation` crate, support multi-codepoint emojis

### M9: Download Force (msg.rs:1164-1192)
Add `--force` flag to allow overwrite

### M10: Progress Indicators (takeout.rs, msg.rs)
Add `indicatif` crate, progress bars for long operations

### M4: Privacy InputUser (privacy.rs:281-285)
Document or fix access_hash: 0 issue

## Branch 4: Low Priority (Issue #5) - 15 Fixes

### Quick Wins (L1-L7)
- L1: Remove duplicate effective_parallel
- L2: Move tele_invocation re-export
- L3: Add tests for raw_message_to_json
- L4: Extract is_sensitive_file
- L5: Fix exit code truncation
- L6: Fix stdout lock in listener
- L7: Safe date parsing

### Architecture (L10-L12)
- L10: Refactor TeleError::Other
- L11: Use log crate macros
- L12: Document phone resolution

### UX Polish (L13-L18)
- L13: Reject --parallel 0
- L14: Standardize --limit defaults
- L15: Add --since/--until filters
- L16: Add --format flag
- L17: Support t.me/+ links
- L18: Implement --silent flag

## Quality Gates
- TDD: Write tests first for behavior changes
- All tests pass (605+)
- Clippy clean (-D warnings)
- Cargo fmt clean
- No comments in code
- Follow AGENTS.md standards

## Deliverables
- 3 branches: fix/security, fix/ux, fix/low-priority
- 3 PRs ready to merge
- All issues closed with resolution comments
