# Hooks Design

## Overview
What runs a plugin without an admin asking. This file lists the hooks that are built and the ones proposed, each with what it delivers, why a plugin needs it, and what it needs granted. It builds on [`requirements/plugins.md` §8](../requirements/plugins.md#8-events) and on [`plugins.md`](plugins.md), which covers how a run executes. Where it conflicts with [`requirements/`](../requirements/), the requirements win.

The one thing this design exists to get right: **a hook tells a plugin what happened, and never holds up, alters, or prevents it** ([`requirements/plugins.md` §8](../requirements/plugins.md#8-events)).

Its companions are [`imports.md`](imports.md), covering what a plugin can call, and [`exports.md`](exports.md), covering what the host can call on a plugin. Everything in §3 is proposed, not decided.

---

## 1. Built Hooks

**Five hooks are built, each approved like a permission** ([`requirements/plugins.md` §4.1](../requirements/plugins.md#41-what-can-be-asked-for)). The dispatcher is `crates/server/src/plugins/hooks.rs`, and the contract is [`wit/plugin.wit`](../crates/plugins/wit/plugin.wit) at `0.3`. Plugins built against `0.2` still load, without `playing` ([`plugins.md` §6](plugins.md#6-packaging)).

| Hook (manifest / WIT) | Delivers | Granted | Also needs |
|---|---|---|---|
| `tracksChanged` / `tracks-changed` | Track IDs changed or removed since the last delivery, up to 100 per event | Per library | `libraryRead` |
| `scanFinished` / `scan-finished` | The summed counts of every scan that finished since the last delivery | Per library | `libraryRead` |
| `schedule` / `scheduled` | Nothing. The interval named by `everyMinutes` (at least 5) came round. | Per plugin | — |
| `played` / `played` | Plays one connected user ended in this library, oldest first, up to 100, each with its track | Per plugin | `libraryRead`, `listeningActivity`, `personalSettings` |
| `playing` / `playing` | The newest play one connected user started in this library, while it is still playing and under a minute old ([§1.1](#11-playing)) | Per plugin | As `played`, and `apiVersion` 0.3 |

Two more events need no approval, because nothing happens without someone asking:

- **`run`**: an admin pressed **Run now**.
- **`check-settings`**: an admin is saving settings, or a user is connecting. An error refuses the save ([`requirements/plugins.md` §6](../requirements/plugins.md#6-configuration--credentials)).

All five hooks share the same delivery rules ([`plugins.md` §7](plugins.md#7-running-plugins)), except that `playing` never catches up ([§1.1](#11-playing)):

- **A position per plugin and library, and per user in `played` and `playing`.** The position moves only once the plugin has handled a batch, so a crash, a restart, or an unreachable service loses nothing.
- **Failures back off.** The retry comes after a minute, doubles up to an hour, and five failures in a row disable the plugin in that library. In `played` and `playing`, one user's failures are retried at most hourly and never disable the plugin for everyone else.
- **One delivery per pair at a time**, polled every 5 seconds.

### 1.1 Playing

**Run when a connected user starts a track.** ListenBrainz's `playing_now` and Last.fm's `updateNowPlaying` both need the start. `played` only arrives once the play has ended. [`plugins/listenbrainz`](../plugins/listenbrainz/) uses it, as an optional permission, to show what is playing now.

- **Delivers** `playing { track, started-at }`: the newest play one connected user started in this library. The run sees that user's personal settings, as in `played`.
- **Where starts come from.** A trigger on `plays` records each play first reported before it ended (`play_starts`, `crates/server/migrations/0014_plugin_playing.sql`). It is timed by the server's clock at that first report, never the device's. A device catching up after being offline reports its plays already ended, so they are never announced as playing. Each new start clears those over an hour old.
- **Not replayed.** "Now playing" from an hour ago is wrong, not late. A start is due only while it is under a minute old and its play has not ended, and the newest passes over every start before it. A failed delivery backs off as any hook's does, so by the retry that start is stale and only a newer one goes. This is the one exception to reliable delivery in [§2](#2-rules-every-hook-follows), and the reason is recorded here so it is not "fixed".
- **Needs** the same grants as `played`: `libraryRead`, `listeningActivity`, and `personalSettings` to connect. It also needs `apiVersion` 0.3, the first contract with a `playing` event.
- **Latency.** Up to the dispatcher's 5-second poll. A wake from the plays endpoint would remove that; it is not built, because that endpoint knows nothing of plugins.
- **Cost.** One run per track started per connected user, which the warm instance in [`plugins.md` §2](plugins.md#2-execution-model) makes cheap. It is not cheap until that instance exists.

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

### 3.1 Searched

**Run when a connected user settles on a search.** This makes possible plugins that act on what someone looked for. Examples: a wishlist of what the user searched for and does not own, an acquisition plugin requesting it on their behalf, or a "did you mean" built from an external catalog.

- **Delivers settled searches, never keystrokes.** Results update on every keystroke ([`requirements/search.md` §7](../requirements/search.md#7-speed)), and a plugin has no use for `b`, `bo`, `bon`. A settled search is the same thing that enters the user's recent searches ([`requirements/search.md` §6](../requirements/search.md#6-recent-searches)), so the server already has to define it.
- **Each search carries:**
  - the query text;
  - the library it searched;
  - how many results each owned section had, so "found nothing" is visible ([`requirements/search.md` §8](../requirements/search.md#8-when-nothing-is-found));
  - the result the user opened or played, if any;
  - when it happened.
- **Searches run on the device count too.** A search the device answered while offline ([`requirements/search.md` §2](../requirements/search.md#2-where-search-runs)) is delivered once it reaches the server with the user's recent searches. Otherwise a plugin would see a different person when they are offline.
- **Needs a new permission, `searchActivity`.** Search history is personal data of its own kind ([`requirements/users.md` §7](../requirements/users.md#7-privacy--personal-data)), and `listeningActivity` does not cover it: every resource gets its own permission ([`requirements/plugins.md` §4.1](../requirements/plugins.md#41-what-can-be-asked-for)). The hook also needs `libraryRead`, to make sense of results. It reaches only users who connected the plugin, so a plugin needs `personalSettings`, as `played` does.
- **The tension with "no permanent search history".** [`requirements/search.md` §6](../requirements/search.md#6-recent-searches) promises "nothing to browse, nothing to audit, nothing to leak". Once a plugin receives a query, it can keep it in `state` or send it to a service. The core cannot then honour clearing. Three proposed mitigations, none sufficient alone:
  1. **The user opts in, separately from connecting.** Their account page says plainly that this plugin receives what they search for and may keep it. Connecting a scrobbler should not quietly also mean sharing searches.
  2. **A `searches-cleared` event** asks the plugin to forget, delivered when the user clears recent searches.
  3. **No plugin surface shows one user's searches to another.** That is the account boundary ([`requirements/plugins.md` §11](../requirements/plugins.md#11-isolation--boundaries)), and it rules out a plugin that reports "what everyone searched for" to the admin.

  This needs a decision in the requirements, not just here ([§5](#5-open-questions)).
- **Not the same as answering searches.** An external source answers queries through the `source` export ([`exports.md` §3.1](exports.md#31-external-content-source)), which is called as the user types and returns results. This hook is called afterwards and returns nothing.

### 3.2 Albums Changed and Artists Changed

**Run when albums or artists are added, changed, or removed.** Artwork and biography plugins think in albums and artists ([`requirements/scanning.md` §3](../requirements/scanning.md#3-sidecar-content)). Today they rebuild those from track IDs and work out for themselves which album changed.

- **Delivers** album or artist IDs changed or removed, as `tracksChanged` delivers track IDs.
- **Built from what exists.** The library change feed already records `album` and `artist` rows (`crates/server/migrations/0004_feeds.sql`). These hooks read it filtered by entity type, with a position of their own.
- **Two hooks, not one `libraryChanged`.** A plugin hears only about what it asked for, and the admin sees "run when albums change" rather than a vaguer grant. The cost is two names in the manifest rather than one.
- **Needs** `libraryRead`. Granted per library.

### 3.3 Scan Problems

**Run when the scanner finds files it could not read or could not make sense of** ([`requirements/scanning.md` §10](../requirements/scanning.md#10-problems)). A tag-fixing plugin wants exactly these files, and `scanFinished` gives only a count.

- **Delivers** each problem's root, path, and kind. These are the same problems an admin sees, as they are found.
- **Needs** `libraryRead`. Fixing a problem needs `libraryChange` as well, but hearing about it does not.
- **Built on** the scanner's problem list, with a position of its own like the change feed. A problem that is fixed and then recurs is delivered again.

### 3.4 User Actions

**Run when a connected user does something worth acting on.** [`requirements/plugins.md` §8](../requirements/plugins.md#8-events) promises "a user action taken", and no hook delivers one yet.

- **One hook, `userActed`, with an enumerated `user-action` variant**, so the contract says exactly which actions exist. Free-form strings would quietly become an unversioned API. First candidates:
  - a playlist created, renamed, or deleted, or tracks added to it or removed from it ([`requirements/playlists.md` §4](../requirements/playlists.md#4-editing)). Example use: mirroring playlists to another service.
  - a context started ([`requirements/queue.md` §1](../requirements/queue.md#1-structure)). Example use: "listening to an album" status for presence plugins.
- **Not every action.** Queue edits and seeking are too frequent and too dull to deliver, and a plugin that wants playback has `played` and `playing`.
- **Needs** a permission for what each action carries. Playlists are personal data, and `listeningActivity` does not cover them. Either the permission list grows (`playlistActivity`), or this hook carries its own permission per action kind. To decide ([§5](#5-open-questions)).

### 3.5 Lifecycle

**Tell a plugin about changes to itself.** These touch nothing but the plugin itself, so they reach nothing ([`requirements/plugins.md` §4.1](../requirements/plugins.md#41-what-can-be-asked-for)).

| Event | Why |
|---|---|
| `enabled` | Enabled in a library: set up `state`, or start a first full pass now rather than at the first schedule. |
| `updated(from-version)` | Migrate `state` written by an older version. Without it, an update breaks quietly on old data. |
| `grants-changed` | [`requirements/plugins.md` §4.3](../requirements/plugins.md#43-enforcement) says "the plugin is told what it has". Today it learns only when a run starts and it calls `granted()`. |
| `user-connected` / `user-disconnected` | Prepare for a user, or delete what was kept for them when they leave ([`requirements/users.md` §7](../requirements/users.md#7-privacy--personal-data)). |

- **Delivered reliably**, since a missed `updated` or `user-disconnected` leaves data the plugin should have dealt with.
- **Whether they need approval.** [`requirements/plugins.md` §8](../requirements/plugins.md#8-events) says every hook is approved like a permission. These reach nothing, so approving them is ceremony. Either they are events, like `run` and `check-settings`, or the requirement says which hooks need approval ([§5](#5-open-questions)).

### 3.6 Clock-Time Schedules

**Run at a time of day, not only every N minutes.** "Every day at 03:00" keeps a whole-library pass off-hours, and `everyMinutes` cannot say that.

- **An extension of `schedule`, not a new hook.** The request takes `at: "03:00"` (server local time) and optionally days of the week, instead of `everyMinutes`. It is still one permission and one approval.
- **The admin can move it.** The plugin proposes a time and the admin decides, as with everything else it asks for.

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

1. **Search activity and permanent history** (§3.1). Whether a plugin may receive searches at all, given [`requirements/search.md` §6](../requirements/search.md#6-recent-searches). If it may, the requirement should say so and what clearing then means.
2. **Which hooks need approval** (§3.5). Lifecycle events reach nothing, and [`requirements/plugins.md` §8](../requirements/plugins.md#8-events) currently says every hook is approved.
3. **Permissions for user actions** (§3.4). One per kind of personal data, or one per action kind.
4. **A position older than the feed's horizon.** The change feed drops tombstones after a retention window. A plugin disabled for longer than that comes back to a position the feed can no longer serve, and a removal it never heard about is lost. That matters to `tracksChanged` now, and to §3.2 when it is built.
5. **Ordering between hooks.** A scan can produce `tracksChanged`, `albumsChanged`, and `scanFinished` together. Whether a plugin can rely on any order between them, or must treat each as independent, should be stated before anyone depends on one.
