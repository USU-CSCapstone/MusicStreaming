# LRCLIB Lyrics

A first-party example plugin (`design/plugins.md` §7). It finds tracks without lyrics, asks
lrclib.net for them, and saves `.lrc` (synced) or `.txt` (plain) files beside the audio, where
the scanner picks them up. Without write access it reports what it found and saves nothing.
It looks up each track as it is added. **Run now**, and the schedule each hour, carry on
through the library from where the last run stopped, looking up each track without lyrics. A
track it saved nothing for, because lrclib.net had nothing, only plain lyrics when synced ones
are asked for, or adding files wasn't granted, is looked up again a month later, the next time
the pass reaches it, unless the saving mode changes first: granting adding files, or turning
synced lyrics only off or on, makes the pass look up again every track it saved nothing for.
Only LRCLIB's `404` counts as having nothing. When it can't be asked now, because it asks to
wait, answers `429` or a `5xx`, doesn't answer in time, or can't be reached, the run stops
there with an error and records nothing for that track: the pass looks it up first next time,
and a change is delivered again. The server retries it with a growing wait, and disables the
plugin only after 72 hours of failing. One whose lookup failed any other way is tried again on
the next way round.
A run stops starting lookups two minutes in, so it ends inside its time limit however large the
library is; the added tracks it had no time for are looked up first by the next run. Its
manifest keeps it to 45 requests a minute, the allowance lrclib.net gives a client that doesn't
identify itself with a homepage link or email ([its docs](https://lrclib.net/docs), Rate
Limiting); the host doesn't send one yet.
Its contract is the host's, [`crates/plugins/wit`](../../crates/plugins/wit), and
`manifest.json` is what the Plugins page shows. How to write one like it is in
[`plugins/README.md`](../README.md).

```sh
rustup target add wasm32-wasip2   # once
./build.sh      # target/lrclib-lyrics.wasm, with the manifest packed in (needs Node)
cargo test      # the LRCLIB logic, natively
```

Then, with the server running:
1. Install `target/lrclib-lyrics.wasm` on the Plugins page, approve its permissions, and enable it.
2. Press **Run now**.
3. Play a song and open **Lyrics** in the player.

Saving needs the music mounted writable, as the development `compose.yaml` mounts it.
The workspace tests run this plugin, so build it before `cargo test --workspace`.
