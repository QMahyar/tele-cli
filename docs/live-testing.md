# Run live tests

Live tests drive real Telegram user accounts through telecli over MTProto. The default `cargo test` run stays fully offline. A live run is opt-in, manual, and bounded by the rules below.

## Know the limits

Telegram punishes automation patterns. See the [spam FAQ](https://telegram.org/faq_spam) and the [error reference](https://core.telegram.org/api/errors) for why these rules exist.

- **FloodWait.** Run sequentially. Keep `--parallel 1`. Space writes at least 5 seconds apart. grammers sleeps through one wait of up to 60 seconds ([AutoSleep](https://docs.rs/grammers-client/latest/grammers_client/client/struct.AutoSleep.html)) and returns longer waits to you. Honor any `FLOOD_WAIT` or `SLOWMODE_WAIT` value you receive.
- **Spam restrictions.** Send only to Saved Messages (`me`). Cap self-sends at 5 per run. Bound each run at 5 minutes. Never message strangers, never post to public groups, never mass forward. Cold outreach earns PeerFlood restrictions and spam reports.
- **Sessions.** Keep one session file per account, under `%APPDATA%\telecli\sessions\` on Windows and `~/.telecli/sessions/` on Linux and macOS. Never share a file between processes.
- **CI.** Never run live tests from CI. Datacenter IPs raise Telegram's abuse score. CI cannot handle interactive login.
- **Third parties.** Joins, cold sends, forwards, contact adds, and group creation need explicit operator approval and disposable accounts. Join at most 1 chat per 10 minutes.

## Run preflight

Preflight comes before every live session. The suite fails loudly on a bad environment. It never re-authenticates on its own.

1. Run `tele account status --json`.
2. Confirm that accounts `1` and `2` both report `"authorized": true`.
3. Ignore an entry named `me` that reports unauthorized. The entry is a known config artifact.
4. Stop on any missing or unauthorized account. Do not continue past a failed preflight.

A failure names the account and the recovery:

```
LIVE TEST PREFLIGHT FAILED
Account '1': session missing or unauthorized
Account '2': OK
Expected accounts: ['1', '2']
Recovery: run 'tele account login' for each missing account, then re-run the suite.
```

To recover a session, log in again and re-check:

```bash
tele account login
tele account status --json
```

If Telegram revoked the auth key but the file still exists, delete the file first.

On Windows:

```
del %APPDATA%\telecli\sessions\<name>.session
```

On Linux and macOS:

```
rm ~/.telecli/sessions/<name>.session
```

Then run `tele account login`. Login is interactive. Enter the phone number, the verification code, and the 2FA password if the account has one. The suite never stores or prompts for 2FA passwords. See [docs/security.md](security.md).

## Run the read-only pass

This phase performs no writes and needs no cleanup. Run it at any time.

Exercise these commands against real sessions:

- `account list`
- `account status --json`
- `dialog list`
- `msg get`
- `msg search`
- `msg download` on a small test file

Takeout qualifies too. `takeout start`, `takeout export`, and `takeout finish` leave chats untouched.

## Write to Saved Messages, then clean up

Every write targets `me`. Cleanup is mandatory, because leftover test messages pollute a real account.

1. Send a marker message with `msg send --to me`.
2. Exercise `msg edit`, `msg react`, `msg pin`, and `msg read` on messages you own.
3. Record every message ID the run creates.
4. Delete all recorded IDs with `msg delete` during teardown. If deletion fails, print the leftover IDs so you can remove them by hand.
5. Restore anything `profile set` touched. Name, bio, and photo return to their prior values.

## Restrict third-party operations

No automated harness runs this tier. Treat it as a manual operation that starts only with explicit operator approval. Use disposable accounts. A primary-account ban disrupts real communication, while a disposable-account ban costs nothing.

When approved:

1. Point sends at one designated test chat.
2. Cap joins at 1 per 10 minutes.
3. Leave every chat the run joins.
4. Log each join, send, and leave with timestamps.

## Gate automated runs

`cargo test` never touches the network. Live checks packaged as Rust integration tests carry two gates. `#[ignore]` on each test is the outer gate. A runtime check of `TELE_LIVE` is the inner gate.

```bash
cargo test                            # offline default, what CI runs
cargo test -- --ignored               # skips cleanly while TELE_LIVE is unset
TELE_LIVE=1 cargo test -- --ignored   # full live suite
```

CI runs plain `cargo test` and never sets `TELE_LIVE`. There is no live-test workflow.

Record each completed run and its results in the commit that ships the fix.

## References

- [ADR-004](decisions/004-flood-and-parallel.md): the sequential-default and parallel-cap decision.
- [docs/security.md](security.md): the secret-handling model.
- [Telegram API errors](https://core.telegram.org/api/errors).
- [Telegram spam FAQ](https://telegram.org/faq_spam).
- [grammers AutoSleep](https://docs.rs/grammers-client/latest/grammers_client/client/struct.AutoSleep.html).
