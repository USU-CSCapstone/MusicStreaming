# Hooks Design

## Overview
What runs a plugin without an admin asking. This file lists the hooks that are built and the ones proposed, each with what it delivers, why a plugin needs it, and what it needs granted. It builds on [`requirements/plugins.md` §8](../requirements/plugins.md#8-events) and on [`plugins.md`](plugins.md), which covers how a run executes. Where it conflicts with [`requirements/`](../requirements/), the requirements win.

The one thing this design exists to get right: **a hook tells a plugin what happened, and never holds up, alters, or prevents it** ([`requirements/plugins.md` §8](../requirements/plugins.md#8-events)).

Its companions are [`imports.md`](imports.md), covering what a plugin can call, and [`exports.md`](exports.md), covering what the host can call on a plugin. Everything in §3 is proposed, not decided.

---

## 1. Built Hooks

**Eight hooks are built, each approved like a permission** ([`requirements/plugins.md` §4.1](../requirements/plugins.md#41-what-can-be-asked-for)). The dispatcher is `crates/server/src/plugins/hooks.rs`, and the contract is [`wit/plugin.wit`](../crates/plugins/wit/plugin.wit) at `0.3`. Plugins built against `0.2` still load, without the hooks 0.3 added ([`plugins.md` §6](plugins.md#6-packaging)).

| Hook (manifest / WIT) | Delivers | Granted | Also needs |
|---|---|---|---|
| `tracksChanged` / `tracks-changed` | Track IDs changed or removed since the last delivery, up to 100 per event | Per library | `libraryRead` |
| `albumsChanged` / `albums-changed`, `artistsChanged` / `artists-changed` | Album or artist IDs changed or removed since the last delivery, up to 100 per event ([§1.3](#13-albums-changed-and-artists-changed)) | Per library | `libraryRead`, `apiVersion` 0.3 |
| `scanFinished` / `scan-finished` | The summed counts of every scan that finished since the last delivery | Per library | `libraryRead` |
| `schedule` / `scheduled` | Nothing. The interval named by `everyMinutes` (at least 5) came round. | Per plugin | — |
| `played` / `played` | Plays one connected user ended in this library, oldest first, up to 100, each with its track | Per plugin | `libraryRead`, `listeningActivity`, `personalSettings` |
| `playing` / `playing` | The newest play one connected user started in this library, while it is still playing and under a minute old ([§1.1](#11-playing)) | Per plugin | As `played`, and `apiVersion` 0.3 |
| `searched` / `searched`, `searches-forgotten` | Searches one user who shares them settled on in this library, up to 100, or those they asked it to forget ([§1.2](#12-searched)) | Per plugin | `libraryRead`, `searchActivity`, `personalSettings`, `apiVersion` 0.3 |

Two more events need no approval, because nothing happens without someone asking:

- **`run`**: an admin pressed **Run now**.
- **`check-settings`**: an admin is saving settings, or a user is connecting. An error refuses the save ([`requirements/plugins.md` §6](../requirements/plugins.md#6-configuration--credentials)).

All eight hooks share the same delivery rules ([`plugins.md` §7](plugins.md#7-running-plugins)), except that `playing` never catches up ([§1.1](#11-playing)):

- **A position per plugin and library, and per user in `played`, `playing`, and `searched`.** The position moves only once the plugin has handled a batch, so a crash, a restart, or an unreachable service loses nothing.
- **Failures back off.** The retry comes after a minute, doubles up to an hour, and five failures in a row disable the plugin in that library. In `played`, `playing`, and `searched`, one user's failures are retried at most hourly and never disable the plugin for everyone else.
- **One delivery per pair at a time**, polled every 5 seconds.

### 1.1 Playing

**Run when a connected user starts a track.** ListenBrainz's `playing_now` and Last.fm's `updateNowPlaying` both need the start. `played` only arrives once the play has ended. [`plugins/listenbrainz`](../plugins/listenbrainz/) uses it, as an optional permission, to show what is playing now.

- **Delivers** `playing { track, started-at }`: the newest play one connected user started in this library. The run sees that user's personal settings, as in `played`.
- **Where starts come from.** A trigger on `plays` records each play first reported before it ended (`play_starts`, `crates/server/migrations/0014_plugin_playing.sql`). It is timed by the server's clock at that first report, never the device's. A device catching up after being offline reports its plays already ended, so they are never announced as playing. Each new start clears those over an hour old.
- **Not replayed.** "Now playing" from an hour ago is wrong, not late. A start is due only while it is under a minute old and its play has not ended, and the newest passes over every start before it. A failed delivery backs off as any hook's does, so by the retry that start is stale and only a newer one goes. This is the one exception to reliable delivery in [§2](#2-rules-every-hook-follows), and the reason is recorded here so it is not "fixed".
- **Needs** the same grants as `played`: `libraryRead`, `listeningActivity`, and `personalSettings` to connect. It also needs `apiVersion` 0.3, the first contract with a `playing` event.
- **Latency.** Up to the dispatcher's 5-second poll. A wake from the plays endpoint would remove that; it is not built, because that endpoint knows nothing of plugins.
- **Cost.** One run per track started per connected user, which the warm instance in [`plugins.md` §2](plugins.md#2-execution-model) makes cheap. It is not cheap until that instance exists.

### 1.2 Searched

**Run when a user who shares their searches with the plugin settles on one, or removes one** ([`requirements/search.md` §6](../requirements/search.md#6-recent-searches)). It makes possible plugins that act on what someone looked for: a wishlist of what they searched for and do not own, an acquisition plugin requesting it for them, or a "did you mean" from an external catalog.

- **Delivers two events, one kind at a time**, so a forget always reaches the plugin after the search it names:
  - `searched(list<search>)`: up to 100 searches, oldest first. Each has its `id`, the words, how many of the user's own tracks, albums, and artists it `found`, the result `selected`, if any, and when.
  - `searches-forgotten(option<list<u64>>)`: the IDs of searches the user removed, or none once they stop sharing, meaning forget every search of theirs from this library.
- **Settled searches only, never keystrokes.** What is delivered is exactly what enters the user's recent searches: one they acted on, submitted, or left after it stood a moment (`crates/server/src/api/recent_searches.rs`). That includes searches that found nothing, with every count zero, which is the case this hook is most often for. The counts are taken from the search index as the search is recorded.
- **Each user turns it on, separately from connecting, and it is off by default.** The admin's grants of `searchActivity` and `searched` make sharing possible; they share nothing on their own. A connected user turns it on per plugin on their account page (`setMySearchSharing`), where they are told plainly that the plugin may keep or send on what they search for, and that removing a search can only ask it to forget ([`requirements/users.md` §7](../requirements/users.md#7-privacy--personal-data)). Only searches from then on are sent.
- **Where they come from.** For a user who shares with any plugin, recording or removing a recent search writes a row to `search_events` (`crates/server/migrations/0016_plugin_searched.sql`). The row names the search; its words live only in `recent_searches`, so one removed before it was delivered is never sent at all. A search replaced by the same words, pushed out by newer ones, or aged out of the 30-day window sends no forget, so a wishlist is not emptied each month. A removed search the plugin was never sent may still be named in a forget, which it can ignore.
- **Stopping reaches the plugin even after disconnecting.** Turning sharing off, or disconnecting, writes a `stopped` row for each library the plugin serves, delivered to that plugin alone. It needs only the hook, since it carries nothing but the request to forget.
- **Delivered as `played` is**: a position per user, failures retried at most hourly, and never disabling the plugin for everyone else. The feed ages out with recent searches, so a plugin unable to take delivery for 30 days misses what it was due (§5, question 3).
- **Needs** `searchActivity`, `libraryRead`, and `personalSettings` to connect, and `apiVersion` 0.3. Search history is personal data of its own kind, which `listeningActivity` does not cover ([`requirements/plugins.md` §4.1](../requirements/plugins.md#41-what-can-be-asked-for)).
- **No plugin surface shows one user's searches to another.** That is the account boundary ([`requirements/plugins.md` §11](../requirements/plugins.md#11-isolation--boundaries)), and it rules out a plugin that reports "what everyone searched for" to the admin.
- **Searches a device answers offline** ([`requirements/search.md` §2](../requirements/search.md#2-where-search-runs)) are to be delivered once they reach the server, so a plugin sees the same person offline as on. Recent searches do not sync from devices yet, so there are none to deliver.
- **Not the same as answering searches.** An external source answers queries through the `source` export ([`exports.md` §3.1](exports.md#31-external-content-source)), which is called as the user types and returns results. This hook is called afterwards and returns nothing.

### 1.3 Albums Changed and Artists Changed

**Run when albums or artists are added, changed, or removed.** Artwork and biography plugins think in albums and artists ([`requirements/scanning.md` §3](../requirements/scanning.md#3-sidecar-content)), and need not rebuild them from track IDs.

- **Delivers** `albums-changed` or `artists-changed`: up to 100 IDs changed or removed, as `tracksChanged` delivers track IDs.
- **What counts as a change** is whatever the library change feed records (`crates/server/src/db/catalog.rs`). An album is recorded as it is recomputed when its tracks change, when its artwork's placeholder changes, and when its loudness is analysed; an artist as it is recomputed or its image changes. A plugin may be told of an album whose title it already has.
- **Read from what exists.** The change feed already recorded `album` and `artist` rows (`crates/server/migrations/0004_feeds.sql`). Each hook reads only its own kind, with a position of its own that starts at zero, so a plugin new to a library first hears of every album or artist in it. `tracksChanged` now reads only tracks the same way, so an album changing no longer wakes a track hook.
- **Two hooks, not one `libraryChanged`.** A plugin hears only about what it asked for, and the admin sees "run when albums change" rather than a vaguer grant. The cost is two names in the manifest rather than one.
- **Needs** `libraryRead`, granted per library, and `apiVersion` 0.3.
- **Read what changed by ID** with `get-albums` and `get-artists`, and where an album is with `album-tracks` ([`imports.md` §1.2](imports.md#12-albums-and-artists)).

---

## 2. Rules Every Hook Follows

Any proposed hook must keep these. A hook that breaks one is rejected on that basis ([§4](#4-rejected-hooks)).

- **A signal, never a veto.** The thing happens first and the plugin hears about it afterwards. Nothing waits on delivery ([`requirements/plugins.md` §2.2](../requirements/plugins.md#22-results-compose-they-do-not-block)).
- **Approved like a permission**, required or optional and with a reason. **It still needs the permission for what it carries.** A hook about a library needs `libraryRead`. A hook about a person needs whatever covers that person's data, and reaches only users who connected the plugin ([`requirements/plugins.md` §4.1](../requirements/plugins.md#41-what-can-be-asked-for)).
- **Delivered reliably, from a position**, by the same machinery as §1, unless being late makes the event worthless (`playing`, [§1.1](#11-playing)). Then it is dropped rather than replayed, and the hook says so.
- **Batched.** One event carries everything since the last, up to a cap, so a burst becomes a few runs, not thousands.
- **It names what changed, not its contents**, where the plugin can read the contents itself. That keeps events small and current, as the change feed does.
- **Library-scoped hooks only ever see their own library** ([`requirements/general.md` §3.6](../requirements/general.md#36-libraries-are-the-isolation-boundary)), and person-scoped hooks only the person they are for ([`requirements/users.md` §7](../requirements/users.md#7-privacy--personal-data)).

---

## 3. Proposed Hooks

### 3.1 Scan Problems

**Run when the scanner finds files it could not read or could not make sense of** ([`requirements/scanning.md` §10](../requirements/scanning.md#10-problems)). A tag-fixing plugin wants exactly these files, and `scanFinished` gives only a count.

- **Delivers** each problem's root, path, and kind. These are the same problems an admin sees, as they are found.
- **Needs** `libraryRead`. Fixing a problem needs `libraryChange` as well, but hearing about it does not.
- **Built on** the scanner's problem list, with a position of its own like the change feed. A problem that is fixed and then recurs is delivered again.

### 3.2 User Actions

**Run when a connected user does something worth acting on.** [`requirements/plugins.md` §8](../requirements/plugins.md#8-events) promises "a user action taken", and no hook delivers one yet.

- **One hook, `userActed`, with an enumerated `user-action` variant**, so the contract says exactly which actions exist. Free-form strings would quietly become an unversioned API. First candidates:
  - a playlist created, renamed, or deleted, or tracks added to it or removed from it ([`requirements/playlists.md` §4](../requirements/playlists.md#4-editing)). Example use: mirroring playlists to another service.
  - a context started ([`requirements/queue.md` §1](../requirements/queue.md#1-structure)). Example use: "listening to an album" status for presence plugins.
- **Not every action.** Queue edits and seeking are too frequent and too dull to deliver, and a plugin that wants playback has `played` and `playing`.
- **Needs** a permission for what each action carries. Playlists are personal data, and `listeningActivity` does not cover them. Either the permission list grows (`playlistActivity`), or this hook carries its own permission per action kind. To decide ([§5](#5-open-questions)).

### 3.3 Lifecycle

**Tell a plugin about changes to itself.** These touch nothing but the plugin itself, so they reach nothing ([`requirements/plugins.md` §4.1](../requirements/plugins.md#41-what-can-be-asked-for)).

| Event | Why |
|---|---|
| `enabled` | Enabled in a library: set up `state`, or start a first full pass now rather than at the first schedule. |
| `updated(from-version)` | Migrate `state` written by an older version. Without it, an update breaks quietly on old data. |
| `grants-changed` | [`requirements/plugins.md` §4.3](../requirements/plugins.md#43-enforcement) says "the plugin is told what it has". Today it learns only when a run starts and it calls `granted()`. |
| `user-connected` / `user-disconnected` | Prepare for a user, or delete what was kept for them when they leave ([`requirements/users.md` §7](../requirements/users.md#7-privacy--personal-data)). |

- **Delivered reliably**, since a missed `updated` or `user-disconnected` leaves data the plugin should have dealt with.
- **Whether they need approval.** [`requirements/plugins.md` §8](../requirements/plugins.md#8-events) says every hook is approved like a permission. These reach nothing, so approving them is ceremony. Either they are events, like `run` and `check-settings`, or the requirement says which hooks need approval ([§5](#5-open-questions)).

### 3.4 Clock-Time Schedules

**Run at a time of day, not only every N minutes.** "Every day at 03:00" keeps a whole-library pass off-hours, and `everyMinutes` cannot say that.

- **An extension of `schedule`, not a new hook.** The request takes `at: "03:00"` (server local time) and optionally days of the week, instead of `everyMinutes`. It is still one permission and one approval.
- **The admin can move it.** The plugin proposes a time and the admin decides, as with everything else it asks for.

### 3.5 Forgetting Plays

**Tell a plugin to forget plays a user cleared.** [`requirements/users.md` §7](../requirements/users.md#7-privacy--personal-data) and [`requirements/analytics.md` §9](../requirements/analytics.md#9-privacy-and-control) require it: clearing listening history asks every plugin that received those plays to forget them. `played` hands plays over, and nothing asks for them back.

- **`plays-forgotten`, on the `played` hook and its grants**, delivered to each user who connected the plugin with the same per-user position. It is the twin of `searches-forgotten` ([§1.2](#12-searched)), and should be built alike.
- **A play needs an ID in the contract.** The `play` record carries none today, so a plugin could not tell which play to forget. Adding `id` is a contract change, so a new version.
- **Waits on clearing history itself.** The API specifies `/me/history/{playId}` and `/me/history/clear`, but the server does not implement them yet.
- **What was sent on stays sent.** A scrobble on ListenBrainz is the user's to delete there, and the user is told so where they connect ([`requirements/users.md` §7](../requirements/users.md#7-privacy--personal-data)). A plugin that can also delete remotely is free to.

---

## 4. Rejected Hooks

**Anything that runs before something happens in order to change it.** For example:

- `before-play`, to block or swap a track;
- `filter-search-results`, to hide or rerank owned results;
- `transform-stream`, for DSP or watermarking audio on its way to a listener;
- `before-scan-ingest`, to alter what the scanner records.

Each would let a plugin veto or alter something ([`requirements/plugins.md` §8](../requirements/plugins.md#8-events)). Each also puts plugin code on a path the core has to wait for, which [§2.2](../requirements/plugins.md#22-results-compose-they-do-not-block) rules out. `before-scan-ingest` would also be a side channel into the database ([`requirements/general.md` §3.2](../requirements/general.md#32-the-scanner-is-the-only-ingestion-path)). A plugin that wants different tags writes different tags to the file.

---

## 5. Open Questions

1. **Which hooks need approval** (§3.3). Lifecycle events reach nothing, and [`requirements/plugins.md` §8](../requirements/plugins.md#8-events) currently says every hook is approved.
2. **Permissions for user actions** (§3.2). One per kind of personal data, or one per action kind.
3. **A position older than the feed's horizon.** The change feed drops tombstones after a retention window. A plugin disabled for longer than that comes back to a position the feed can no longer serve, and a removal it never heard about is lost. That matters to `tracksChanged`, `albumsChanged`, `artistsChanged`, and `searched`.
4. **Ordering between hooks.** A scan can produce `tracksChanged`, `albumsChanged`, and `scanFinished` together. Whether a plugin can rely on any order between them, or must treat each as independent, should be stated before anyone depends on one.
