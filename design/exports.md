# Exports Design

## Overview
What the host can call on a plugin. This file lists the export that is built and the ones proposed, each with what the host calls it for, what it must return, and what it needs granted. It builds on [`requirements/plugins.md`](../requirements/plugins.md) and on [`plugins.md`](plugins.md), which covers how a run executes. Where it conflicts with [`requirements/`](../requirements/), the requirements win.

The one thing this design exists to get right: **the host trusts nothing a plugin returns.** Every entity it names, every surface it describes, and every stream it offers is checked against the run's library and grants before any client sees it ([`requirements/general.md` §3.6](../requirements/general.md#36-libraries-are-the-isolation-boundary), [§3.7](../requirements/general.md#37-payloads-carry-only-what-the-client-needs)).

Its companions are [`hooks.md`](hooks.md), covering what runs a plugin, and [`imports.md`](imports.md), covering what a plugin can call. Everything in §3 is proposed, not decided.

---

## 1. The Built Export

**One function: `handle(event) -> result<string, string>`** in [`wit/plugin.wit`](../crates/plugins/wit/plugin.wit) at `0.2`. Every run goes through it: a hook ([`hooks.md` §1](hooks.md#1-built-hooks)), **Run now**, or a settings check. It returns a one-line summary on success or a reason on failure, which admins see with the run.

This covers everything a plugin does in the background. It covers none of what a plugin does while someone waits, because nothing in the contract is called on a user's behalf and answered back to them. [`requirements/plugins.md` §9](../requirements/plugins.md#9-extending-the-interface) and [§10](../requirements/plugins.md#10-external-content-sources) need exactly that.

---

## 2. Rules Every Export Follows

- **Every call is bounded** ([`requirements/plugins.md` §2.2](../requirements/plugins.md#22-results-compose-they-do-not-block)). An export called while a user waits has a deadline of seconds, not the minutes a background run gets. A plugin that misses it is dropped for that call, and its section says so.
- **Core results never wait.** The core answers within its own budget, and plugin contributions fill in after. A slow plugin delays only itself.
- **Stale calls are cancelled.** Once a user has typed the next character or left the page, the host stops waiting for the previous answer and discards it ([`requirements/search.md` §7](../requirements/search.md#7-speed)).
- **What comes back is checked.** Entity IDs must belong to the run's library, external content is labelled by the host and not the plugin, and URLs and credentials never pass through to a client.
- **Errors never reach users** ([`requirements/plugins.md` §11](../requirements/plugins.md#11-isolation--boundaries)). A failing export means a section saying "unavailable", and a record for the admin.

---

## 3. Proposed Exports

The WIT below is a sketch of each export's shape. Types it names without defining are left to the real design.

### 3.1 External Content Source

**Music the user does not own, searchable, browsable, queueable, and playable alongside music they do** ([`requirements/plugins.md` §10](../requirements/plugins.md#10-external-content-sources)).

```wit
interface source {
  /// Opaque to the host; namespaced by the plugin's ID before any client sees it.
  type item-id = string;

  record item {
    id: item-id,
    kind: item-kind,          // track, album, artist, playlist
    title: string,
    artists: list<string>,
    album: option<string>,
    duration-ms: option<u64>,
    artwork: option<item-id>, // fetched through `artwork`, never a URL handed to a client
  }

  search: func(query: string, limit: u32) -> result<list<item>, string>;
  browse: func(node: option<item-id>, after: option<string>) -> result<page, string>;
  get: func(ids: list<item-id>) -> result<list<item>, string>;
  open: func(id: item-id) -> result<stream, string>;
  artwork: func(id: item-id) -> result<list<u8>, string>;
}
```

- **`search` is called as the user types**, in its own labelled results section ([`requirements/search.md` §1](../requirements/search.md#1-what-is-searched)). The host waits briefly after each keystroke before calling, so a remote service sees something like a word, not each letter. This is the busiest export, and it needs the warm instances and pools in §4.
- **`browse`** follows whatever structure the source has, a page at a time.
- **`get`** re-reads items by ID. A queue or playlist holding external entries ([`requirements/queue.md` §1](../requirements/queue.md#1-structure)) uses it to show them again later.
- **`open` returns how to play an item.** The answer is either a request for the host to send, under the plugin's `network` grant and declared destinations, or a handle the host reads in ranges. **The host proxies the audio** and transcodes it as it would any track ([`requirements/playback.md` §1](../requirements/playback.md#1-delivery)), so upstream URLs, tokens, and the service's identity never reach a client ([`requirements/general.md` §3.7](../requirements/general.md#37-payloads-carry-only-what-the-client-needs)).
- **The host enforces the rules in [§10](../requirements/plugins.md#10-external-content-sources), so plugins cannot get them wrong:**
  - every item is labelled external, with its source;
  - external items never feed recommendations ([§10.3](../requirements/plugins.md#103-its-limits));
  - they are never offered for download;
  - a source that fails says it is down rather than returning nothing.
- **Needs a new permission, `externalSource`**, per library, since it shows up in that library's search. It also needs `network`.

### 3.2 Interface

**A plugin's own sections, actions, and pages** ([`requirements/plugins.md` §9](../requirements/plugins.md#9-extending-the-interface)). The format both clients render is still undecided ([`general.md` §11](general.md#11-open-decisions) #4, [`plugins.md` §8](plugins.md#8-open-questions) #2). These are the calls that format would travel in.

```wit
interface ui {
  /// Sections on an existing page, such as a biography on an artist page.
  sections: func(entity: entity-ref) -> result<list<section>, string>;
  /// Runs an action the manifest declared, on the entities it was chosen for.
  invoke: func(action: string, entities: list<entity-ref>) -> result<action-outcome, string>;
  /// A page of the plugin's own, at a route under the plugin.
  page: func(route: string, params: list<tuple<string, string>>) -> result<view, string>;
}
```

- **A description, not code.** A plugin returns a tree of the client's own components: lists, entity rows, text, images, buttons bound to its actions. It never returns HTML or script. Only then can a surface follow the client's layout and input rules at every size and with every input ([`requirements/plugins.md` §9](../requirements/plugins.md#9-extending-the-interface)). Script would also be another party's code running in the client.
- **Actions are declared in the manifest, not discovered by a call.** A context menu has to open instantly ([`requirements/conventions.md` §2](../requirements/conventions.md#2-actions)), so the client must know a plugin's actions before asking it anything. `invoke` runs when an action is chosen, and it may take as long as it needs, with its progress shown.
- **Every entity a plugin names is checked.** An ID in a section or a view that is not in the run's library is dropped before rendering. Without that check, this export is how a library leaks.
- **Offline, plugin surfaces are shown as unavailable rather than hidden** ([`requirements/offline.md`](../requirements/offline.md)). A plugin may mark a section as cacheable with the entity it describes, such as a biography. That is a later refinement.
- **Needs** nothing to add a surface ([`requirements/plugins.md` §4.1](../requirements/plugins.md#41-what-can-be-asked-for)). What the surface shows needs whatever grants reading it needs.

### 3.3 Connect

**Connecting through a service's own sign-in, not by pasting a key.** `personalSettings` only collects typed values. Last.fm, Spotify, and Discogs instead connect by sending the user to sign in on the service and receiving a callback.

```wit
interface connect {
  /// Where to send the user. `callback` is the host's URL for this plugin and user.
  begin: func(callback: string) -> result<string, string>;
  /// The callback's query parameters; returns the personal settings to keep.
  finish: func(params: list<tuple<string, string>>) -> result<list<tuple<string, value>>, string>;
}
```

- **The host owns the callback route** and binds it to the user who started the flow. The plugin never learns of any other user, and cannot complete a connection for someone who did not start one.
- **What `finish` returns is stored as personal settings**, written once and never displayed ([`requirements/plugins.md` §6](../requirements/plugins.md#6-configuration--credentials)). Holding them means being connected, as now.
- **Needs** `network` for the token exchange, and `personalSettings` declared in the manifest.

### 3.4 Structured Outcomes

**`handle` returns a record rather than a string.** Admins already see per-plugin timings ([`requirements/plugins.md` §2.2](../requirements/plugins.md#22-results-compose-they-do-not-block)). A record also lets them see what each run did.

```wit
record outcome {
  summary: string,
  /// Work left over; the host runs it again soon rather than at the next hook.
  more: bool,
  /// Named counts the admin sees, such as ("lyrics saved", 15).
  counts: list<tuple<string, u64>>,
}
```

- **`more` is what makes long jobs resumable without the plugin running for an hour** ([`requirements/plugins.md` §7](../requirements/plugins.md#7-working-with-the-library)). A plugin works in bounded runs, keeping its place in `state` ([`imports.md` §3.1](imports.md#31-requirements-not-yet-met)). The host keeps scheduling runs, yielding to listeners between them.

---

## 4. Structure

**A WIT world cannot have optional exports**, and most plugins fill only one role. So the contract splits into **role worlds** that each `include` a base world:

| World | Exports |
|---|---|
| `plugin` | `handle`, as now |
| `source` | `handle` and `source` |
| `ui` | `handle` and `ui` |
| `connect` | `handle` and `connect` |

The base world keeps every import.

- **A plugin that fills several roles is built against a world that includes several.** The host reads which exports a component has at install. That is a fact about the component, as imports will be ([`plugins.md` §6](plugins.md#6-packaging)). The manifest does not declare roles.
- **Version `0.3` adds the role worlds and keeps `0.2` linking.** A plugin written against `0.2` must keep working ([`requirements/plugins.md` §1](../requirements/plugins.md#1-an-open-surface)). The host links both until `0.2` is retired by a documented deprecation.
- **`source` and `ui` are called on hot paths, which background runs never were.** They depend on work [`plugins.md` §7](plugins.md#7-running-plugins) lists as not done:
  - one warm instance per plugin and library, rather than a fresh instance per run;
  - a small pool per busy plugin, since one instance serializes its calls ([`plugins.md` §8](plugins.md#8-open-questions) #5);
  - run requests that return before the run finishes.
  
  Building these exports before that work would make every keystroke pay instantiation, and every user's search wait behind every other user's.

---

## 5. Open Questions

1. **Sending queries to an external source.** Every user who types in a library with a source enabled has their typing sent to a third party. The admin enabled the source, but the user did not. Whether a user must opt in, or at least be told, needs deciding in the requirements. The same decision covers the `searched` hook ([`hooks.md` §3.2](hooks.md#32-searched)).
2. **What a playlist keeps of an external entry.** If its source goes away, the entry stays in place, unplayable, and explained ([`requirements/plugins.md` §10.2](../requirements/plugins.md#102-it-behaves-like-music)). Explaining it needs at least a title and artist, so something is stored. That sits uneasily with "nothing was stored" ([§10.1](../requirements/plugins.md#101-it-is-not-library-content)). The likely answer is a display snapshot the playlist owns, not the source.
3. **The interface format** (§3.2). This is the same decision as [`general.md` §11](general.md#11-open-decisions) #4.
4. **Deadlines per export.** How long a section or a search may take before it is dropped. This should be stated as a number in [`requirements/performance.md`](../requirements/performance.md), not chosen here.
