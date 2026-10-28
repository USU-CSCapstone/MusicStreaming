# Organizing your music

Jewelcase builds your library from the tags inside your music files. Folder names don't set
anything: an album is whatever tracks share an album tag, wherever they are. A file name is
used only as the title of a track that has none. The other things Jewelcase finds by name are
the extra files beside your music, such as cover art and lyrics.

If something looks wrong, fix the tags with a tagger such as
[MusicBrainz Picard](https://picard.musicbrainz.org) or [beets](https://beets.io). Jewelcase
picks up changes on its own.

## Checklist

- Give every track a **title, artist, album, album artist, and track number**, plus a **disc
  number** on multi-disc albums.
- Separate several artists or genres with **semicolons**: `Daft Punk; Pharrell Williams`.
- Give each album **cover art**, embedded in the files or as `cover.jpg` in the album's folder.
- On compilations, set the album artist to `Various Artists` and set the **compilation** flag.
- To tell editions of an album apart, give them **different album titles**.
- Keep **one copy** of each album.
- Arrange folders as **artist, then album**: `Music/Artist/Album/tracks`.

## Formats

FLAC, MP3, AAC and ALAC (`.m4a`, `.mp4`, `.m4b`, `.aac`), Ogg Vorbis, Opus, WAV, and AIFF.
WMA isn't supported. Other files, such as documents and images, are ignored. A supported file
that can't be read is left out of the library; the log counts these but doesn't name them yet.

Your browser plays the file as it is, so a track in a format it can't play is skipped for now.
ALAC and AIFF, for example, play in Safari but not in most other browsers.

## Folders

**We recommend one folder per artist, with one folder per album inside it:**
`Music/Artist/Album/tracks`. Jewelcase doesn't require it, since tags decide everything, but it
keeps the library tidy, and it gives each album's cover art and each artist's image and
biography an obvious place, so they're found for the right albums.

```
Music/
└── Radiohead/
    ├── artist.jpg                    artist image
    ├── artist.txt                    artist biography
    └── OK Computer/
        ├── cover.jpg                 album art
        ├── 01 Airbag.flac
        ├── 01 Airbag.lrc           lyrics for 01 - Airbag.flac
        └── 02 Paranoid Android.flac
```

- **Compilations** fit the same pattern under their album artist: `Music/Various Artists/Album/`.
- **Symbolic links are skipped**, files and folders alike.
- **Moving or renaming a file, or a folder it's in, makes it a new track** for now. The old one
  stays listed as unavailable for 30 days, with its play history. Retagging a file in place is
  fine.

## Tags

Jewelcase reads the same fields from every format. The names below are the ones most taggers
show, and the same tags in MP3 and `.m4a` files are read too.

| Field | Tag | How it's used |
|---|---|---|
| Title | `TITLE` | The track's name. Without one, the file name is used. |
| Artist | `ARTIST` | Who performs the track. The first artist is the main credit. |
| Album | `ALBUM` | Which album the track is on |
| Album artist | `ALBUMARTIST` | Who the album belongs to. Without one, the track's first artist. |
| Track number | `TRACKNUMBER`, `TRACKTOTAL` | Order on the disc, and how many tracks that disc has. `3/12` works too. |
| Disc number | `DISCNUMBER`, `DISCTOTAL` | Which disc. `1/2` works too. |
| Date | `DATE` | Release date, as precise as tagged: `1997`, `1997-05`, or `1997-05-21`. Without one, `YEAR`, `RELEASEDATE`, or `ORIGINALDATE`. |
| Genre | `GENRE` | The track's genres. Albums and artists take theirs from their tracks. |
| Compilation | `COMPILATION` | `1` marks a compilation |
| Release type | `RELEASETYPE` | `album`, `single`, `ep`, or `compilation` ([below](#album-types)) |
| Label | `LABEL` | The record label. Without one, `PUBLISHER`. |
| Sort names | `TITLESORT`, `ARTISTSORT`, `ALBUMSORT`, `ALBUMARTISTSORT` | Sorting, such as `Bowie, David` to sort under B. A leading "The", "A", or "An" is ignored without one. |
| Lyrics | `LYRICS` | Plain or time-synced (LRC) lyrics |
| Cover art | embedded picture | The front cover is preferred |

Jewelcase also reads ISRC and MusicBrainz IDs. It ignores ReplayGain tags, since it measures
each track's loudness itself.

**Multiple values** are separated by semicolons, or stored as repeated fields where the format
allows. Nothing else splits a name: `Simon & Garfunkel` and `Earth, Wind & Fire` are one artist
each, and `feat.`, commas, and `/` are left as written. To credit a guest, add them as a second
artist.

**Names match regardless of letter case,** so `Radiohead` and `RadioHead` are one artist, shown
as most tracks spell it. Other differences, such as extra spaces or punctuation, make separate
artists or albums.

**Missing tags are fine.** A file with no tags is still indexed, named after its file, under an
unknown artist and album.

### Picard users

Two of Picard's defaults don't suit Jewelcase yet:

- **Picard writes a combined name in `ARTIST`**, such as `Daft Punk feat. Pharrell Williams`,
  and the separate names in `ARTISTS`. Jewelcase reads `ARTIST` and uses `ARTISTS` only when
  `ARTIST` is empty, so that track gets one combined artist. To credit each artist, put the
  names in `ARTIST`, separated by semicolons.
- **Picard writes several release types**, such as `album` and then `compilation`. Jewelcase
  reads only the first, so a compilation can be typed as an album.

## Albums

**An album is its title plus its album artists.** Tracks that share both are one album, even
across folders or drives. Tracks with different album titles are different albums, even in the
same folder.

- **Set the album artist.** Without one, each track counts its first artist as the album
  artist, so an album whose tracks have different lead artists splits into several.
- **Compilations:** set the album artist to `Various Artists` and set the compilation flag, so
  the album doesn't split by artist. `Various Artists` is an ordinary name, so it shows in your
  artist list. A compilation with no album artist is filed under the unknown artist.
- **Editions** of an album with the same title and album artist merge. Tag them apart, such as
  `OK Computer` and `OK Computer OKNOTOK 1997 2017`.
- **Duplicates** aren't detected yet. A FLAC and an MP3 copy of the same album, tagged the same,
  make one album with every song twice.
- **Track order** is by disc number, then track number. The track totals come from tags, so an
  album with 3 of its 12 tracks shows as incomplete rather than as a 3-track release. Give each
  disc its own total, not the whole album's.

### Album types

Each album is typed as an album, EP, single, or compilation. Jewelcase decides in this order:

1. **The release type tag**, when more than half the album's tracks agree.
2. **The compilation flag** makes it a compilation.
3. **The tagged track total:** 1–3 tracks is a single, 4–6 an EP, and 7 or more an album.
4. **Otherwise, an album.**

### Artists

**An artist's albums are the ones they're the album artist of.** Albums where they're only a
track artist, such as for a guest verse, show under **Appears On** on their page instead, so
guests never fill up someone's discography.

## Extra files

Jewelcase looks for these files beside your music. File names are matched in any case.

| File | Where | Used for |
|---|---|---|
| `cover.jpg` | The track's folder, or any folder above it | Album art, when the files have none embedded |
| `artist.jpg` | The track's folder, or any folder above it | The album artist's image |
| `artist.txt` | The track's folder, or any folder above it | The album artist's biography, as plain text |
| `<file name>.lrc` or `.txt` | The same folder as the track | Lyrics, when the file has none embedded |

- **Images** can be `.jpg`, `.jpeg`, `.png`, `.webp`, `.gif`, or `.avif`. If a folder has
  several, they're tried in that order.
- **The nearest file wins.** Jewelcase looks in the track's own folder first, then each folder
  above it, stopping at the top of the library. A `cover.jpg` in an album folder beats one in
  the artist folder above it.
- **Only put `artist.jpg` and `artist.txt` in a folder that holds one artist's albums.** They go
  to each album's album artist, never to guests, and one at the top of the library would give
  every artist the same picture.
- **Lyrics match by file name**, only in the track's own folder: `01 - Airbag.flac` pairs with
  `01 - Airbag.lrc`, and `.lrc` is tried before `.txt`. Files with timestamps (LRC) scroll in
  time with the music; others show as plain text.
- **Embedded art and lyrics win** over files.
- **Empty files, and files over 32 MB, are ignored.**
- **Changes are picked up on their own**, including adding, replacing, or removing these files.

These names are also how plugins add content: the lyrics plugin, for example, saves `.lrc` or
`.txt` files next to your tracks.
