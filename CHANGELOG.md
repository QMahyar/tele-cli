# Changelog

## [Unreleased]

### Changed
- Per-account flood weights: each account now gets its own token-bucket rate limiter
  (`rpc_per_minute`) and flood cooldown (`flood_sleep_threshold`), replacing the
  single global semaphore. A flooded account no longer blocks siblings.
- `--parallel` clamp raised to 1..=32 (was 1..=3); default remains 1.
- New config keys under `[accounts.<name>]`: `rpc_per_minute: f64` (token-bucket
  budget; `None` = unlimited) and `flood_sleep_threshold: u64` (per-account
  AutoSleep threshold; `None` = global default).
- ADR-007 supersedes ADR-004 for flood/parallel design.
