# Ticket: Slice 3 — Tier A features

Type: `wayfinder:task` (AFK execution)
Branch: `slice/3-features`
Blocks: (none — last)
Question: Land the highest-value competitor features.

Scope (Tier A, from competitor research):
1. Bulk/album download + resume: `msg download --all|--since|--album` with checkpoint (download.rs:61).
2. Voice/video-note send: `msg send --file x.ogg --as voice|video-note` (send.rs:498).
3. Poll creation: `msg send --poll` (vote exists, create doesn't).
4. Edit media + caption: `msg edit --file/--caption/--format/--no-preview` (msg/mod.rs:122).
5. Search filters: `--from/--kind/--since/--until` on `msg search` (search maps to messages.search).

Each feature: update `docs/capabilities.md` in the same change; RED test → GREEN.

Done = clippy + `cargo test` green; capabilities matrix updated.
