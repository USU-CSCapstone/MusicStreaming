# LRCLIB Lyrics

A first-party example plugin (`design/plugins.md` §7). It finds tracks without lyrics, asks
lrclib.net for them, and saves `.lrc` (synced) or `.txt` (plain) files beside the audio, where
the scanner picks them up. Without write access it reports what it found and saves nothing.
Its contract is [`crates/plugins/wit/plugin.wit`](../../crates/plugins/wit/plugin.wit), and
`manifest.json` is what the Plugins page shows.

```sh
./build.sh      # target/lrclib-lyrics.wasm, with the manifest packed in (needs Node)
cargo test      # the LRCLIB logic, natively
```

Then, with the server running:
1. Install `target/lrclib-lyrics.wasm` on the Plugins page, approve its permissions, and enable it.
2. Press **Run now**.
3. Play a song and open **Lyrics** in the player.

Saving needs the music mounted writable; the development `compose.yaml` mounts it read-only.
The host's own tests run this plugin once it is built:
`cargo test -p jewelcase-plugins -- --ignored`.
