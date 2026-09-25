# Requirements

Twenty documents defining what Jewelcase must do. Project overview and core principles: root [`CLAUDE.md`](../CLAUDE.md).

**Every fact lives in exactly one file; the rest cite it as a link, `` [`file.md` §N](file.md#n-heading) ``.** Find the home before writing.

## The files

| File | Covers |
|---|---|
| [`general.md`](general.md) | Goals, non-goals, core principles, target platforms. The root of everything else. |
| [`conventions.md`](conventions.md) | Behavior shared by all entities — personal data, actions, rendering, sorting, imagery, availability, multi-artist display. |
| [`libraries.md`](libraries.md) | Library definition, storage, administration, isolation, change feed. |
| [`scanning.md`](scanning.md) | Ingestion — formats, tag and sidecar rules, triggers, track identity, file lifecycle. |
| [`users.md`](users.md) | Accounts, roles, auth, devices, per-library access. |
| [`tracks.md`](tracks.md) | Track entity, metadata, audio properties, lyrics. |
| [`albums.md`](albums.md) | Album entity, identity, type, artwork, grouping. |
| [`artists.md`](artists.md) | Artist entity, images, biographies, ownership model. |
| [`tags.md`](tags.md) | Tag and genre modeling, normalization. |
| [`playlists.md`](playlists.md) | Creation, mutation, concurrency control, artwork, export. |
| [`queue.md`](queue.md) | Queue structure, shuffle, repeat, sessions, cross-device behavior. |
| [`playback.md`](playback.md) | Streaming, transcoding, waveform, seeking, gapless, loudness, playback state. |
| [`search.md`](search.md) | Matching, ranking, scoping, recent searches. |
| [`recommendations.md`](recommendations.md) | Similarity, radio and endless play, generated mixes, rediscovery. |
| [`analytics.md`](analytics.md) | Plays, listening history, statistics, period summaries. |
| [`realtime.md`](realtime.md) | Shift — live updates, remote control, playback transfer. |
| [`offline.md`](offline.md) | Catalog sync, downloads, local-versus-stream, offline determinism. |
| [`plugins.md`](plugins.md) | The open surface, cost constraints, trust, lifecycle, external content. |
| [`performance.md`](performance.md) | Verification hardware, budgets, benchmarks, saturation behavior. |
| [`deployment.md`](deployment.md) | Container deployment, configuration, backup, upgrades, remote access, observability. |

## Where cross-cutting things live

The lookups where the obvious file isn't the owning one.

| Looking for | It lives in |
|---|---|
| Anything true of *every* entity — actions, rendering, sorting, availability | [`conventions.md`](conventions.md), not the entity file |
| What "instant" or "at scale" means as a number | [`performance.md` §3](performance.md#3-interaction-budgets)–[§6](performance.md#6-client-footprint) |
| Sidecar conventions (`cover.*`, `artist.*`, `artist.txt`, `.lrc`) | [`scanning.md` §3](scanning.md#3-sidecar-content) — a **public contract** plugins depend on |
| Artwork *found* vs *served* | [`scanning.md` §3.1](scanning.md#31-album-art) vs [`conventions.md` §5](conventions.md#5-imagery) |
| Track identity, and why moving or retagging files is safe | [`scanning.md` §6](scanning.md#6-track-identity) |
| Missing files — marking, retention, what pins a track | [`scanning.md` §8](scanning.md#8-missing-files) |
| Missing vs not-downloaded, and skipping in place | [`conventions.md` §6](conventions.md#6-availability) — nothing else restates it |
| Whether an artist owns music or merely appears on it | [`artists.md` §2](artists.md#2-ownership-discography-vs-appearances) |
| Semicolons as the only multi-value delimiter | [`artists.md` §1](artists.md#1-identity), echoed in [`tags.md` §3](tags.md#3-multiple-values) |
| What plays next vs how audio gets there | [`queue.md`](queue.md) vs [`playback.md`](playback.md) |
| Whether a local copy or a stream is used | [`offline.md` §4](offline.md#4-choosing-between-local-and-stream) |
| Version-checked edits and refusal on conflict | [`playlists.md` §5](playlists.md#5-concurrent-edits), consequence in [`realtime.md` §7](realtime.md#7-conflicting-commands) |
| Which preferences follow the account vs the device | [`users.md` §6.1](users.md#61-where-preferences-live) |
| Inaccessible content being indistinguishable from nonexistent | [`users.md` §10](users.md#10-access-semantics) |
| The bound on what a plugin may cost | [`plugins.md` §2](plugins.md#2-a-fast-system-not-restricted-plugins) |
| Rules for music the user does not own | [`plugins.md` §10](plugins.md#10-external-content-sources) |

## Vocabulary

Used precisely. Don't introduce synonyms.

- **Context** — anything playable as a run of tracks: album, artist, playlist, tag page, search results, filtered view, whole library, radio station.
- **Manual Queue / Context Queue / History** — the three parts of a queue.
- **Saved vs generated playlist** — both are "playlists" in the interface. A **filtered view** is neither and leaves nothing behind.
- **Shift** — using any of your devices to control any of your others.
- **Catalog / imagery / audio** — the three things held on a device, with different sync rules.
- **Available vs playable** — present on the server vs present on this device.
- **External content** — music a plugin surfaces that the user doesn't own. Never library content.

## Conventions

- **Structure.** `# <Area> Requirements`, a `## Overview` naming the one thing the file exists to get right, then `---`-separated numbered sections.
- **Section numbers and headings are an interface.** Links carry the heading as an anchor, so `grep -rn "file.md#" .` before renumbering, retitling, or removing a section, and fix every link that pointed at it.
- **Assert, then justify.** Lead with a **bolded claim**, follow with the reasoning in the same breath. Rationale is part of the requirement — a rule whose reason isn't recorded gets re-argued or quietly violated.
- **State the trade.** Where a requirement costs something, say so and say why it's worth it.
- **Name the failure mode** for requirements at risk of being built wrong, so "wrong" is recognizable: a waveform that renders as a flat line, an offline search that is a weaker search.
- **One home per fact.** Needed in three places means it belongs in [`conventions.md`](conventions.md) or [`general.md`](general.md).
- **Requirements, not designs.** "Search tolerates typos" is a requirement; "search uses trigram indexing" is not.
