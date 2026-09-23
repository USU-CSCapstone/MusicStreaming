# Requirements

Twenty documents defining what Jewelcase must do. Project overview and core principles: root `CLAUDE.md`.

**Every fact lives in exactly one file; the rest cite it as `` `file.md` §N ``.** Find the home before writing.

## The files

| File | Covers |
|---|---|
| `general.md` | Goals, non-goals, core principles, target platforms. The root of everything else. |
| `conventions.md` | Behavior shared by all entities — personal data, actions, rendering, sorting, imagery, availability, multi-artist display. |
| `libraries.md` | Library definition, storage, administration, isolation, change feed. |
| `scanning.md` | Ingestion — formats, tag and sidecar rules, triggers, track identity, file lifecycle. |
| `users.md` | Accounts, roles, auth, devices, per-library access. |
| `tracks.md` | Track entity, metadata, audio properties, lyrics. |
| `albums.md` | Album entity, identity, type, artwork, grouping. |
| `artists.md` | Artist entity, images, biographies, ownership model. |
| `tags.md` | Tag and genre modeling, normalization. |
| `playlists.md` | Creation, mutation, concurrency control, artwork, export. |
| `queue.md` | Queue structure, shuffle, repeat, sessions, cross-device behavior. |
| `playback.md` | Streaming, transcoding, waveform, seeking, gapless, loudness, playback state. |
| `search.md` | Matching, ranking, scoping, recent searches. |
| `recommendations.md` | Similarity, radio and endless play, generated mixes, rediscovery. |
| `analytics.md` | Plays, listening history, statistics, period summaries. |
| `realtime.md` | Shift — live updates, remote control, playback transfer. |
| `offline.md` | Catalog sync, downloads, local-versus-stream, offline determinism. |
| `plugins.md` | The open surface, cost constraints, trust, lifecycle, external content. |
| `performance.md` | Verification hardware, budgets, benchmarks, saturation behavior. |
| `deployment.md` | Container deployment, configuration, backup, upgrades, remote access, observability. |

## Where cross-cutting things live

The lookups where the obvious file isn't the owning one.

| Looking for | It lives in |
|---|---|
| Anything true of *every* entity — actions, rendering, sorting, availability | `conventions.md`, not the entity file |
| What "instant" or "at scale" means as a number | `performance.md` §3–§6 |
| Sidecar conventions (`cover.*`, `artist.*`, `artist.txt`, `.lrc`) | `scanning.md` §3 — a **public contract** plugins depend on |
| Artwork *found* vs *served* | `scanning.md` §3.1 vs `conventions.md` §5 |
| Track identity, and why moving or retagging files is safe | `scanning.md` §6 |
| Missing files — marking, retention, what pins a track | `scanning.md` §8 |
| Missing vs not-downloaded, and skipping in place | `conventions.md` §6 — nothing else restates it |
| Whether an artist owns music or merely appears on it | `artists.md` §2 |
| Semicolons as the only multi-value delimiter | `artists.md` §1, echoed in `tags.md` §3 |
| What plays next vs how audio gets there | `queue.md` vs `playback.md` |
| Whether a local copy or a stream is used | `offline.md` §4 |
| Version-checked edits and refusal on conflict | `playlists.md` §5, consequence in `realtime.md` §7 |
| Which preferences follow the account vs the device | `users.md` §6.1 |
| Inaccessible content being indistinguishable from nonexistent | `users.md` §10 |
| The bound on what a plugin may cost | `plugins.md` §2 |
| Rules for music the user does not own | `plugins.md` §10 |

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
- **Section numbers are an interface.** `grep -rn "file.md" .` before renumbering or removing one.
- **Assert, then justify.** Lead with a **bolded claim**, follow with the reasoning in the same breath. Rationale is part of the requirement — a rule whose reason isn't recorded gets re-argued or quietly violated.
- **State the trade.** Where a requirement costs something, say so and say why it's worth it.
- **Name the failure mode** for requirements at risk of being built wrong, so "wrong" is recognizable: a waveform that renders as a flat line, an offline search that is a weaker search.
- **One home per fact.** Needed in three places means it belongs in `conventions.md` or `general.md`.
- **Requirements, not designs.** "Search tolerates typos" is a requirement; "search uses trigram indexing" is not.
