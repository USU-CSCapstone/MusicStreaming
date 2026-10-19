# LRCLIB Lyrics

A first-party example plugin (`design/plugins.md` §7). It finds tracks without lyrics, asks
lrclib.net for them, and saves `.lrc` (synced) or `.txt` (plain) files beside the audio, where
the scanner picks them up. Without write access it reports what it found and saves nothing.
It looks up each track as it is added. **Run now**, and the schedule each hour, carry on
through the library from where the last run stopped, looking up each track without lyrics. A
track it saved nothing for, because lrclib.net had nothing, only plain lyrics when synced ones
are asked for, or adding files wasn't granted, is looked up again a month later, or sooner once
adding files is granted or plain lyrics are wanted again, the next time the pass reaches it. One
whose lookup failed is tried again on the next way round. A run stops starting lookups two
minutes in, so it ends inside its time limit however large the library is.
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

Saving needs the music mounted writable, as the development `compose.yaml` mounts it.
The workspace tests run this plugin, so build it before `cargo test --workspace`.
