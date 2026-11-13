# MusicBrainz Tagger

A first-party example plugin (`design/plugins.md` §7). It finds tracks with no artist, searches
MusicBrainz for the recording, and writes the tags the file is missing (title, artist, album
artist, album, track and disc number, date) and its release's front cover into the file, where
the scanner picks them up. Without access to replace files it reports what it found and writes
nothing.

**It reads titles the way people write them.** A song saved from a video is often titled
`Travis Scott feat. Future, Young Thug & M.I.A. - FRANCHISE (REMIX - Official Audio)`. What
video sites add, such as `(Official Audio)`, `[Lyrics]`, or `- Official Video`, is taken out,
`(feat. …)` is read as featured artists, and a leading track number from a file name, `01 - `
or `01. `, is dropped. A title that names its artist is searched as the song by the artist before
any `feat.`, with a length within 15 seconds, since a saved video often runs a little long; a
title alone is searched with a length within 3 seconds. The rules are in
[`src/title.rs`](src/title.rs).

**It only fills in blanks.** A tag the file already has is never overwritten, with one
exception: a title that isn't the song's own, as above, is replaced by the song's. A cover is
embedded only in a file with no picture, rather than saved as `cover.jpg`, which would apply to
every track in the folder (`requirements/scanning.md` §3.1). Covers come from the Cover Art
Archive, at 500 pixels across, whose images are served from archive.org servers with changing
names; that is why the manifest asks to reach any host.

**It writes nothing it isn't sure of, unless the title says it.** A title and a length alone
can't tell two artists' songs apart, so when recordings by more than one artist match, it logs
who they could be by and leaves the file alone. When MusicBrainz has no match, or more than one
artist's, for a title that names its artist, it writes that title and those artists as the
title reads, with no album or cover; the **Guess from the title** setting turns that off. Of the
releases a match is on, it picks an official album over an EP over a single, one that isn't a
compilation or live, and then the earliest, so the original album wins over reissues and
compilations. **Embed covers** turns covers off.

It looks up each track as it is added. **Run now**, and the schedule each hour, carry on through
the library from where the last run stopped. A track it wrote nothing for, because there was no
match, more than one artist's, or replacing files wasn't granted, is looked up again a month
later, or as soon as replacing files is granted or taken away. When MusicBrainz can't be asked
now, because it asks to wait, answers `429` or a `5xx`, doesn't answer in time, or can't be
reached, the run stops there with an error, and the server retries it with a growing wait. A
search it turns away as too many, with a `503` or `429`, is sent once more first, after the
pause the host keeps after one.
A run stops starting lookups, and fetching covers, a minute in, so it ends inside its time
limit however large the library is. Its manifest keeps it to 50 requests a minute, a little
under the one a second MusicBrainz allows, since requests sent a second apart can arrive closer
together, and every request names the plugin in its `User-Agent`.

**Files over 16 MiB are skipped**, with a line in the log: writing tags holds the file and
copies of it in memory, which must fit in a plugin's 64 MiB. Most MP3s fit; many FLACs don't.

Its contract is the host's, [`crates/plugins/wit`](../../crates/plugins/wit), and
`manifest.json` is what the Plugins page shows. How to write one like it is in
[`plugins/README.md`](../README.md).

```sh
rustup target add wasm32-wasip2   # once
./build.sh      # target/musicbrainz-tagger.wasm, with the manifest packed in (needs Node)
cargo test      # the matching and tagging logic, natively
```

Then, with the server running:
1. Install `target/musicbrainz-tagger.wasm` on the Plugins page, approve its permissions, and
   enable it.
2. Press **Run now**, and open **Log** to see what it matched.
3. The tracks it tagged move under their artist and album as the scanner reads the files it
   wrote, which it does on its own.

Writing needs the music mounted writable, as the development `compose.yaml` mounts it.
