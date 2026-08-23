# W5-1: stickers.manage — pack management

**Branch:** `feat/w5-1-stickers` · **Files:** NEW `src/commands/stickers.rs`, `src/commands/mod.rs`, `src/main.rs`, `src/serialize.rs`(additive helpers only if needed)

## Goal

Sticker/GIF pack management per matrix row (send-as-sticker already works via send --file today).

## Acceptance

- [ ] Verify TL surface at layer 227 first: messages.{GetAllStickers, GetStickers, SearchStickers, InstallStickerSet, UninstallStickerSet, ...} + stickers.SuggestedShortName. Design the command around what EXISTS; honest report for gaps
- [ ] Proposed surface (adjust to findings, keep flat-subcommand style): `tele sticker list [--query Q]`, `tele sticker install SET`, `tele sticker remove SET [--archive]`, `tele sticker sets [--hash]` — pick naming that matches inventory conventions (`sticker` singular group like `topic`)
- [ ] Rows: short_name, title, count, installed/archived flags where available; mutators honor explicit --account + --dry-run
- [ ] New module registered in mod.rs + main.rs Command enum + contract fixture map entries (tests/contract.rs pattern exists for new groups — follow CAP-era precedent)
- [ ] Offline tests: validation matrices, row shaping fixtures; gates green

## Boundaries

No other command files.
