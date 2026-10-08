# ListenBrainz

A first-party example plugin (`design/plugins.md` §7). Each user who connects it with their
own ListenBrainz token has their plays sent to their account as they end, once half the
track or four minutes was heard, as ListenBrainz asks. Where an admin also approves the
optional `playing` hook, what they start is shown on their profile as playing now; that is
never sent late, so one ListenBrainz turns away is not sent again. The token is checked with
ListenBrainz as it is saved. A play ListenBrainz turns away is kept and sent again later. Its contract is
[`crates/plugins/wit/plugin.wit`](../../crates/plugins/wit/plugin.wit), and `manifest.json`
is what the Plugins page shows.

```sh
./build.sh      # target/listenbrainz.wasm, with the manifest packed in (needs Node)
cargo test      # what counts as a listen, and the submission, natively
```

Then, with the server running:
1. Install `target/listenbrainz.wasm` on the Plugins page, approve its permissions, and enable it.
2. On **Account**, connect ListenBrainz with the token from listenbrainz.org/settings.
3. Play something through to the end; it appears on your ListenBrainz profile within seconds.

The server's own tests run this plugin against ListenBrainz once it is built:
`cargo test -p jewelcase-server real_scrobbler -- --ignored`.
