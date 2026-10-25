# ListenBrainz

A first-party example plugin (`design/plugins.md` §7). Each user who connects it with their
own ListenBrainz token has their plays sent to their account as they end, once half the
track or four minutes was heard, as ListenBrainz asks. Where an admin also approves the
optional `playing` hook, what they start is shown on their profile as playing now; that is
never sent late, so one ListenBrainz turns away is not sent again. The token is checked with
ListenBrainz as it is saved. A play that can't be sent now, because ListenBrainz can't be
reached, refuses the token, or asks to wait, is kept and sent again later. One it would refuse
for good is left out and counted in the run's summary: a play with no title or artist, or one
ListenBrainz turns away as invalid. ListenBrainz refuses a whole batch for one invalid play, so
a refused batch is split in halves and sent again until only the plays it refuses are left out.
[ListenBrainz asks](https://listenbrainz.readthedocs.io/en/latest/users/api/index.html)
each client application for at most one call a second, and says a request with a valid user
token may get a higher limit. Its manifest keeps each connected user's requests to one a second,
apart from other users', which paces the halves sent again too.
Its contract is the host's, [`crates/plugins/wit`](../../crates/plugins/wit), and
`manifest.json` is what the Plugins page shows. How to write one like it is in
[`plugins/README.md`](../README.md).

```sh
rustup target add wasm32-wasip2   # once
./build.sh      # target/listenbrainz.wasm, with the manifest packed in (needs Node)
cargo test      # what counts as a listen, and the submission, natively
```

Then, with the server running:
1. Install `target/listenbrainz.wasm` on the Plugins page, approve its permissions, and enable it.
2. On **Account**, connect ListenBrainz with the token from listenbrainz.org/settings.
3. Play something through to the end; it appears on your ListenBrainz profile within seconds.

The workspace tests run this plugin, so build it before `cargo test --workspace`.
