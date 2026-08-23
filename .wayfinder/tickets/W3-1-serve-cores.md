# W3-1: serve-B — action cores extraction + pipe dispatch

**Branch:** `feat/w3-1-serve-cores` · **Files:** `src/commands/{serve,msg,chat,dialog,account,topic,contact,profile,privacy}.rs`, `src/serialize.rs`, possibly small touches in `src/client.rs` · **SOLO WAVE — no other agents run concurrently**

## Goal

Make `tele serve` able to EXECUTE actions, not just stream: every op request runs against the already-connected ClientGuard in-process (no re-connect, no session lock contention).

## Acceptance

- [ ] Extract per-command cores from fanout closures: `*_core(client,&ClientGuard fields..., params) -> TeleResult<Value>` for at least: msg send/edit/delete/react/get/forward/pin/read/search/vote/typing/click; keep clap handlers calling the same cores (zero behavior change — existing suites must stay green untouched)
- [ ] Serve protocol v1.1 (additive): ops use command-path form ("msg send", "msg react", ...) matching envelope.command vocabulary (TERMINOLOGY LOCK in map-wants.md); params = the command's JSON-able args subset; each op returns response_ok(id, same data shape as CLI --json) or response_err(id, error envelope incl. FLOOD_WAIT seconds)
- [ ] Ops execute against held guard via cores + guard.rate_limiter.acquire(); dry_run param honored per-op; unknown op → NotImplemented envelope; malformed params → ServeError envelope naming the field
- [ ] ping stays; hello unchanged; EOF semantics unchanged; docs/cli-contract.md NOT edited (manager syncs docs at merge)
- [ ] Offline tests: dispatch table mapping, params-parse errors, NotImplemented fallback, at least one core roundtrip via existing fixture seams (no network)
- [ ] Gates green across FULL suite

## Boundaries

Do not change wire shapes already shipped (hello/response/event rows). Do not edit docs/, tasks/, .wayfinder/. If a core extraction forces a public signature change in serialize.rs or client.rs, keep it additive.
