# BUG-3: contact add discards RPC result

**Source:** deep-dive dialog-group H3.

## Question/Problem

`src/commands/contact.rs:149-161` sends `contacts.AddContact` but discards the returned Updates containing the updated `User`. Output is always `"added": true` even when: re-adding silently renames an existing contact, or the peer's privacy settings blocked adding entirely.

## Acceptance criteria

- [ ] Parse the returned user from the Updates response; emit additive JSON fields `contact: bool` and `mutual: bool` reflecting actual post-add state.
- [ ] When the response shows the peer was NOT added as contact (privacy), report honestly (non-zero partial/failure semantics consistent with executor rule; human output states it).
- [ ] Warn (stderr log_line) when overwriting an existing contact name.
- [ ] Update `docs/cli-contract.md` additively for new JSON fields.
- [ ] Contract/unit tests for the shaping logic offline (seam or fixture-based).

## Verification

- [ ] clippy -D warnings / fmt check / full `cargo test`

## Files

- `src/commands/contact.rs`, `docs/cli-contract.md`, tests

## Constraints

Branch `fix/bug-3-contact-result`. Additive JSON only. No comments.
