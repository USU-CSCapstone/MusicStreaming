# Plugin Requirements

## Overview
The core serves a library, quickly, and does little else. Everything beyond that — richer metadata, artwork, lyrics, scrobbling, content from elsewhere, and a hundred things nobody has thought of — is a plugin's to build.

**The plugin system does not enumerate what a plugin may be.** There is no fixed list of capabilities, no approved categories, and no core release required to make a new kind of plugin possible. A user who wants Jewelcase to do something it does not do should be able to make it do that, without asking the project's permission and without waiting for it.

That openness has exactly one price, and it is not negotiable: **a plugin may do anything, but it may never make Jewelcase slow.** The system is unbounded in *kind* and bounded in *cost* ([§2](#2-a-fast-system-not-restricted-plugins)).

Shared behavior follows [`conventions.md`](conventions.md).

---

## 1. An Open Surface

**If the server knows it or does it, a plugin can reach it.** The useful shorthand: *anything a third-party client can do through the public API ([`general.md` §4](general.md#4-target-platforms)), a plugin can do* — plus the server-side hooks that let it act on its own rather than waiting to be asked.

Things people will build early, offered as a starting point and explicitly **not** the boundary:

- **Metadata and artwork enrichment** — artist images, biographies, album art, corrected tags ([§7](#7-working-with-the-library)).
- **Lyrics**, timed or plain, for tracks that have none ([`tracks.md`](tracks.md)).
- **Scrobbling** to Last.fm, ListenBrainz, and anything similar ([`analytics.md` §10](analytics.md#10-external-services)), built on events ([§8](#8-events)).
- **External content sources** — music the user does not own, browsable and playable alongside music they do ([§10](#10-external-content-sources)).
- **Acquisition** — placing new music into the library, which the scanner then treats like anything else ([`general.md` §2](general.md#2-non-goals)).
- **Automation and reporting** — scheduled work, health checks, exports, backups, notifications.
- **Interface surfaces** of their own ([§9](#9-extending-the-interface)).

What keeps this open rather than merely long:

- **Plugins define their own data, settings, and surfaces.** A plugin needing to store something, expose a setting, or add a page does not need the core to have anticipated it.
- **New kinds of plugin never require a core release.** If adding a category of extension means changing Jewelcase itself, the system is not open — it is a menu, and the menu is always missing the thing someone actually wants.
- **The plugin API is a public, documented, stable contract**, versioned like the client API, so a plugin written today keeps working.
- **A plugin can be trivial.** Fifty lines that rename something on a schedule is a legitimate plugin, and the system must not demand ceremony proportional to nothing.

---

## 2. A Fast System, Not Restricted Plugins

**The system's job is to add no cost of its own and to make a well-written plugin fast. How fast any particular plugin is, is its author's business.**

### 2.1 The System Adds Nothing

- **Invoking a plugin costs approximately nothing** beyond the work the plugin itself does. No meaningful dispatch cost, no serialization tax, no per-call startup.
- **Plugins stay warm.** A plugin invoked on every keystroke does not pay a cold start each time.
- **Plugin code reaches library data directly** — never round-tripping through the public API to the server it is already running inside, and never one entity at a time when it wants a thousand. Most slow plugins are slow because the system permitted an access pattern it should have made easy to avoid.
- **Independent work runs concurrently.** The system never serializes plugin work that has no reason to be serial, and one plugin's slowness never queues behind another's.
- **Shared caching is provided**, so expensive results and repeated remote lookups are not recomputed — and two plugins asking the same question ask it once.
- **Plugins keep persistent state**, so nothing already learned is fetched twice.

### 2.2 Results Compose, They Do Not Block

- **Core results never wait on plugin results.** A search returns the library's matches within its own budget ([`performance.md` §3](performance.md#3-interaction-budgets)); external sections fill in as their sources answer.
- **A slow plugin delays only its own contribution.** Its section shows that it is still working; nothing else on the screen is affected, and no other plugin is held up behind it.
- **Every plugin call is bounded.** A plugin that never answers is dropped and its section says so. **The user always gets an answer, even when a plugin does not.**
- **Background plugin work yields to listeners** ([`performance.md` §7](performance.md#7-under-saturation)). Enrichment surrenders resources to playback, always.
- **Cost is attributable.** Admins see per-plugin timings and failure rates, so "search feels slow" resolves to a named plugin rather than a suspicion — and a plugin author can find out why.

---

## 3. Writing to the Library

**A plugin may write anything: new files, new audio, changes to existing files, deletions.** It is the only component permitted to, and the permission is unrestricted.

This qualifies a promise made in [`general.md` §1](general.md#1-goals), and is stated plainly rather than buried:

> **The core never touches your files. A plugin you install can do anything to them.**

That is the trade, made knowingly at install time ([§5](#5-installation--lifecycle)). Restricting plugins to sidecar files would rule out the two things they are most wanted for: correcting bad tags in place, and bringing new music in.

- **Everything a plugin writes is ingested by the scanner** ([`general.md` §3.2](general.md#32-the-scanner-is-the-only-ingestion-path)). There is no side channel into the database, so a plugin-fetched biography and a hand-placed one are indistinguishable.
- **Nothing a plugin writes is privileged.** It can be corrected, overwritten, or deleted by the user like any other file.
- **The core's own guarantee is unchanged.** Uninstall Jewelcase with no plugins installed and the collection is exactly as it was.
- **Backups are the real safety net**, and the documentation says so where plugins are installed rather than leaving users to discover it.

---

## 4. Trust

**Installing a plugin is the approval.** There is no permission model, no capability gate, and no sandbox promise the project cannot actually keep — and an open system could not honestly offer one, since it cannot know in advance what a plugin will need.

- **Only admins install plugins** ([`users.md` §8](users.md#8-admin-capabilities--visibility)). Ordinary users can neither add nor enable one.
- **The risk is stated where the decision is made** — at the point of installing, in plain language.
- **No registry, no curation, no signing.** Plugins are installed by dropping in a file or pasting a link. The project does not maintain a list of blessed plugins, because curation implies a vetting it is not doing.
- **A plugin is as trusted as the admin who installed it** — the framing every self-hosted tool arrives at honestly: this is software you chose to run on your own machine.

---

## 5. Installation & Lifecycle

- **Installation is trivial** — a file or a URL, from the admin interface, with no restart and no shell.
- **Enabled per library, independently** ([`libraries.md`](libraries.md)). A plugin may serve one library and never see another.
- **Disabling is immediate and complete.** A disabled plugin runs nothing, sees nothing, and reaches nothing.
- **Uninstalling leaves the library alone.** Files a plugin wrote are ordinary files and stay; content it merely *sourced* disappears with it ([§10](#10-external-content-sources)).
- **Updates are the admin's choice**, never automatic — code with unrestricted write access must not change underneath a running server.
- **Configuration survives a reinstall**, so removing a plugin to fix something does not mean setting it up again.

---

## 6. Configuration & Credentials

- **Plugins declare their own configuration**, and it is presented consistently however unusual the plugin.
- **Per plugin, per library** where the two differ, so one library can use a different source than another.
- **Credentials are stored securely and never displayed again** after entry, never logged, and never returned by the API.
- **Where a source is personal, credentials are per user.** A plugin serving a household must not force everyone onto one account.
- **Configuration is validated when it is entered.** A wrong key is reported immediately, not silently at three in the morning during a scan.

---

## 7. Working With the Library

Rules for plugins that read and improve what the user owns:

- **Existing metadata is not overwritten blindly.** Whether a plugin may replace metadata already present is the admin's choice — "improve my tags" and "leave my careful tagging alone" are both legitimate.
- **Long jobs are bounded and resumable.** Enriching a 500,000-track library survives restarts and does not begin again from nothing.
- **Rate limits are respected.** A plugin that hammers an external service until it is banned has broken something the user cannot fix.
- **Results appear as the scanner finds them** ([`scanning.md` §5](scanning.md#5-behavior)), with no separate refresh for the user to perform.

---

## 8. Events

- **Plugins are notified of what happens on the server** — a track played, a scan completed, content added or removed, a user action taken.
- **Play events carry enough detail to be acted on accurately**, scrobbling included ([`analytics.md` §10](analytics.md#10-external-services)).
- **Events are delivered reliably.** A plugin briefly unreachable receives what it missed, so a network hiccup does not silently lose a day of scrobbles.
- **Events never block anything** ([§2](#2-a-fast-system-not-restricted-plugins)). Playback, scanning, and every user action complete regardless of whether a plugin is listening, slow, or broken.
- **Events are signals, not vetoes.** A plugin is told what happened; it does not get to prevent or alter it.

---

## 9. Extending the Interface

**A plugin can add to the interface, not just to the data behind it** — its own pages, its own sections, its own actions on existing entities. A plugin that can fetch something but never show it is only half a plugin.

- **Plugin surfaces are real surfaces**, navigable and actionable like anything the core provides ([`conventions.md` §2](conventions.md#2-actions)).
- **They follow the client's layout and input rules** — every size, every input, touch and mouse and keyboard alike — so the app stays coherent across platforms rather than becoming a collection of foreign panels.
- **They fill in rather than block** ([§2.2](#22-results-compose-they-do-not-block)). A plugin's section may take as long as its source takes; the page around it renders immediately regardless.
- **A failing plugin surface degrades locally** — its own section says something useful and the rest of the screen is untouched ([§11](#11-isolation--boundaries)).

---

## 10. External Content Sources

**A plugin can make music playable that the user does not own.** The problem is concrete: someone else wants to hear a song that is not in the collection, and today that means abandoning the queue and opening another app. Jewelcase should be the only app open.

### 10.1 It Is Not Library Content

**External content never enters the library, and is never scanned, indexed, or owned.** The scanner remains the only ingestion path ([`general.md` §3.2](general.md#32-the-scanner-is-the-only-ingestion-path)) precisely because this content does not use it — it is not ingested at all, only surfaced.

- **It is always visibly distinguishable from owned music**, wherever it appears, and labelled with its source. A user must always be able to tell what is theirs. That is the difference between a library and a search box.
- **Removing the plugin removes the content.** Nothing is left behind, because nothing was stored.
- **It is scoped to a library** like everything else ([`general.md` §3.6](general.md#36-libraries-are-the-isolation-boundary)), appearing only where the plugin is enabled.

### 10.2 It Behaves Like Music

Within those bounds it is first-class, because content a user has to treat differently is content they will not use.

- **Searchable**, in its own clearly labelled section of results ([`search.md` §1](search.md#1-what-is-searched)) — never mixed into the sections describing what the user owns.
- **Browsable** through whatever structure the source offers — artists, albums, playlists.
- **Queueable anywhere**, alongside owned tracks in the same queue ([`queue.md` §1](queue.md#1-structure)). A mixed queue is the entire point.
- **Playable with the same controls** — the same player, the same transport, the same Shift behaviour across devices ([`realtime.md`](realtime.md)).
- **Recorded in listening history and statistics**, marked as external ([`analytics.md` §1](analytics.md#1-what-is-recorded)). It is still listening, and omitting it would make a user's own history wrong.
- **Addable to playlists**, marked as external. If the source goes away the entry behaves exactly like a missing track — held in place, unplayable, explained ([`playlists.md` §3](playlists.md#3-contents)).

### 10.3 Its Limits

- **It cannot be downloaded** and does not work offline, presented like any other track whose audio cannot play ([`conventions.md` §6](conventions.md#6-availability)).
- **It never drives recommendations** ([`recommendations.md` §1](recommendations.md#1-what-can-be-recommended)), so a guest's choices in the car do not reshape what the owner is offered for weeks afterward.
- **Availability is never guaranteed.** Sources vanish, change terms, and rate-limit; a source that is down says so plainly rather than appearing empty.

---

## 11. Isolation & Boundaries

A plugin is third-party code with unrestricted write access. The server's obligation is that its failures stay its own.

- **A failing plugin never degrades the server.** Crashing, hanging, or erroring affects that plugin's features and nothing else — playback, scanning, browsing, and search are unaffected.
- **A plugin that keeps failing is disabled and reported**, rather than retrying forever against a service that is gone.
- **Failures are visible to admins** — what failed, when, and why — so a plugin that quietly stopped working is discoverable rather than mysterious.
- **Users are never shown plugin errors.** Missing artwork is missing artwork, not a stack trace.

The only things a plugin genuinely may not do, and each is a correctness rule rather than a limit on ambition:

- **Cross the account boundary.** Listening history, playlists, and queues are private between users ([`users.md` §7](users.md#7-privacy--personal-data)).
- **Cross the library boundary** into libraries it is not enabled for ([`general.md` §3.6](general.md#36-libraries-are-the-isolation-boundary)).
- **Grant itself anything.** A plugin cannot enable itself, widen its own reach, or install another.
