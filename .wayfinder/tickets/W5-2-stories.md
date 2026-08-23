# W5-2: stories.* — story surface

**Branch:** `feat/w5-2-stories` · **Files:** NEW `src/commands/stories.rs`, `src/commands/mod.rs`, `src/main.rs`, contract fixtures

## Goal

Ship the stories.* want row around what layer 227 offers (survey first, honesty gaps).

## Acceptance

- [ ] Survey tl/api.tl: stories.{SendStory, EditStory, DeleteStory, GetStoriesArchive, GetStoriesByID, ReadStories, TogglePinned, ...} + inputMedia payloads; design flat subcommands to EXISTING methods only
- [ ] Likely shape: `tele story send --chat X --file F [--caption]`, `story list --chat X [--archive]`, `story read --chat X --ids`, `story delete --chat X --ids`, `story pin/unpin` — adjust to reality; peer restrictions documented honestly (stories target users/own channel per API rules)
- [ ] Mutators: explicit --account + --dry-run; media upload follows msg.rs upload patterns (reuse validate_upload_path)
- [ ] Register module + Command enum + contract fixture entries (stickers precedent from W5-1)
- [ ] Offline tests: validation matrices, row shaping; gates green

## Boundaries

No other command files beyond registration points.
