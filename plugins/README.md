# Writing a plugin

A Jewelcase plugin is a WebAssembly component with a manifest packed inside it. It runs
sandboxed inside the server, and can only reach what an admin approves: reading the library,
saving files into it, calling web services, or hearing what users play and search. Fetching
lyrics, scrobbling, and fixing tags can all be plugins.

This guide gets a plugin built and running, then explains how plugins work, every manifest
option, and the limits a plugin runs within. Three complete plugins are in this folder to copy
from:

- [`lrclib-lyrics`](lrclib-lyrics/) works through the whole library a part at a time, saves
  lyrics files, and retries when its service is down.
- [`listenbrainz`](listenbrainz/) acts for each user who connects it, with their own token.
- [`musicbrainz-tagger`](musicbrainz-tagger/) reads and rewrites files, filling in the tags
  untagged tracks are missing.

> [!NOTE]
> The plugin contract is at version 0.1 and will change before the first release.

## Quick start

This builds a plugin that counts the tracks in a library when an admin presses **Run now**.

### What you need

- [Rust](https://rustup.rs) with the WebAssembly target: `rustup target add wasm32-wasip2`
- [Node](https://nodejs.org), to pack the manifest into the plugin
- A checkout of this repository, for the contract and the pack tool
- A running Jewelcase to install it on ([Getting started](../README.md#getting-started))

### 1. Create the project

```
hello/
├── Cargo.toml
├── manifest.json
├── src/
│   └── lib.rs
└── wit/          copied from crates/plugins/wit in this repository
```

```sh
cargo new --lib hello
cp -r /path/to/jewelcase/crates/plugins/wit hello/wit
cd hello
```

Create it outside the Jewelcase checkout, or add an empty `[workspace]` table to its
`Cargo.toml`, since the checkout is a Cargo workspace of its own.

`Cargo.toml`:

```toml
[package]
name = "hello"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib", "rlib"]

[target.'cfg(target_arch = "wasm32")'.dependencies]
wit-bindgen = "0.62"

[profile.release]
strip = true
```

`cdylib` builds the plugin, and `rlib` lets the same code build natively for tests.

### 2. Write the manifest

`manifest.json` says who the plugin is and what it needs, with a reason for each permission
that the admin reads before approving it:

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "apiVersion": "0.1",
  "description": "Counts the tracks in a library.",
  "permissions": [
    { "permission": "libraryRead", "required": true, "reason": "To count your tracks." }
  ]
}
```

### 3. Write the plugin

`src/lib.rs`:

```rust
//! Counts the tracks in a library when an admin presses Run now.

#[cfg(target_arch = "wasm32")]
mod plugin {
    wit_bindgen::generate!({ path: "wit", world: "plugin" });

    use jewelcase::plugin::{host, library};

    struct Plugin;

    impl Guest for Plugin {
        fn handle(event: Event) -> Result<String, String> {
            let Event::Run = event else {
                return Ok("Nothing to do.".into());
            };
            let mut count = 0;
            let mut after = None;
            loop {
                let page = library::tracks(after, 500).map_err(|e| format!("{e:?}"))?;
                let Some(last) = page.last() else { break };
                after = Some(last.id);
                count += page.len();
                host::log(&format!("Counted {count} tracks so far"));
            }
            Ok(format!("Hello! Tracks in this library: {count}."))
        }
    }

    export!(Plugin);
}
```

`generate!` reads the contract in `wit/` and creates the Rust types and functions for it. The
plugin's one job is `handle`: it gets an event, does its work through the host's functions,
and returns a one-line summary, or an error saying what went wrong.

### 4. Build and pack

```sh
cargo build --release --target wasm32-wasip2
node /path/to/jewelcase/tools/plugin-pack/pack.mjs \
  target/wasm32-wasip2/release/hello.wasm manifest.json -o hello.wasm
```

`hello.wasm` is the finished plugin, about 80 KB. Cargo turns hyphens in a package name into
underscores, so a package called `my-plugin` builds `my_plugin.wasm`. The examples keep these
two commands in a `build.sh` ([`lrclib-lyrics/build.sh`](lrclib-lyrics/build.sh)).

### 5. Install and run it

1. In Jewelcase, open **Plugins** at the bottom of the sidebar, which only admins see.
2. Under **Add a plugin**, choose `hello.wasm`, or drop it on the page.
3. In the dialog that opens, press **Approve all**. That grants what it asks for and enables
   it in every library.
4. Press **Run now**. The summary appears under the plugin; open **Log** to see its log
   lines.

If the manifest has a mistake, the install is refused with a message saying what to fix.

## How plugins work

### Runs and events

Every run is one call to `handle(event)` for one library. The library is fixed for the run, so
the plugin never names one, and can't reach a library it isn't enabled in.

| Event | When it comes | Permission it needs |
|---|---|---|
| `run` | An admin pressed **Run now** | None |
| `check-settings` | An admin is saving settings, or a user is connecting the plugin. Return an error to refuse them. Checks of server-wide or personal settings run outside any library, with no `state`. | None |
| `tracks-changed` | Tracks were added, changed, or removed, in batches | `tracksChanged` |
| `albums-changed` | Albums were added, changed, or removed | `albumsChanged` |
| `artists-changed` | Artists were added, changed, or removed | `artistsChanged` |
| `scan-finished` | Scans finished, with what they found | `scanFinished` |
| `scheduled` | Once when it's enabled, then every `everyMinutes` | `schedule` |
| `played` | A connected user's plays ended, whether finished, skipped, or stopped | `played` |
| `playing` | A connected user started a track. Dropped, not retried, once over a minute old. | `playing` |
| `searched` | A user who shares their searches settled on some | `searched` |
| `searches-forgotten` | A user asked for searches they shared to be forgotten | `searched` |

A plugin receives only the events it asked for and the admin approved. Each hook keeps a
saved position, so no change is missed, even across restarts. A change event names each item's
latest state, not every edit along the way, and a plugin new to a library first hears of
everything already in it. One plugin runs one event at a time in each library.

### What a plugin can call

| Interface | Functions | Permission |
|---|---|---|
| `library` | `tracks`, `albums`, `artists` (pages in ID order), `get-tracks`, `get-albums`, `get-artists` (by ID), `album-tracks`, `artist-albums` | `libraryRead` |
| `files` | `roots`, `entries`, `read` | `libraryRead` |
| | `write` with `WriteMode::Create`, for a new file | `libraryAdd` |
| | `write` with `WriteMode::Replace`, `rename`, `delete` | `libraryChange` |
| `http` | `send` | `network`, to the manifest's `destinations` only |
| `state` | `get`, `set`, `delete`: storage kept between runs, per library | None |
| `settings` | `get`: the plugin's settings, and the user's own in a user's run | None |
| `host` | `log` a progress line; `granted` lists the permissions this run has | None |

In Rust, hyphens become underscores: `get-tracks` is `library::get_tracks`. The full contract,
with every type, is [`crates/plugins/wit/plugin.wit`](../crates/plugins/wit/plugin.wit).

**Optional permissions can be declined.** The functions stay available but answer
`not-granted`. Call `host::granted()` at the start of a run to adapt.

### Common tasks

These go inside the `plugin` module, with
`use jewelcase::plugin::{files, http, library, settings, state};`.

**Read a setting.** Values come back as `Text`, `Number` (integers too), or `Flag`:

```rust
let synced_only = matches!(settings::get("syncedOnly"), Some(settings::Value::Flag(true)));
```

**Keep something between runs.** `state` stores bytes, per plugin and library:

```rust
let cursor: Option<u64> = state::get("cursor")
    .map_err(|e| format!("{e:?}"))?
    .and_then(|bytes| bytes.try_into().ok())
    .map(u64::from_le_bytes);
state::set("cursor", &track.id.to_le_bytes()).map_err(|e| format!("{e:?}"))?;
```

**Call a web service.** The host must be in the manifest's `destinations`:

```rust
let reply = http::send(&http::Request {
    method: http::Method::Get,
    url: "https://example.com/api/lookup".into(),
    headers: vec![("accept".into(), "application/json".into())],
    body: Vec::new(),
})
.map_err(|e| format!("{e:?}"))?;
if reply.status != 200 {
    return Err(format!("example.com answered {}", reply.status));
}
let body = String::from_utf8_lossy(&reply.body);
```

**Save a file beside a track.** A track's `root` and `path` say where its file is. Files a
plugin saves are scanned in like any other change, so this is how a plugin adds lyrics or
artwork. The names Jewelcase looks for are in
[Organizing your music](../docs/organizing-music.md#extra-files).

```rust
let lyrics = match track.path.rsplit_once('.') {
    Some((stem, _)) => format!("{stem}.lrc"),
    None => format!("{}.lrc", track.path),
};
match files::write(track.root, &lyrics, text.as_bytes(), files::WriteMode::Create) {
    Ok(()) | Err(files::Error::Failed(_)) => {} // saved, or already there
    Err(e) => return Err(format!("{e:?}")),
}
```

For more, [`listenbrainz/src/lib.rs`](listenbrainz/src/lib.rs) uses settings and HTTP, and
lrclib's [`src/plugin/pass.rs`](lrclib-lyrics/src/plugin/pass.rs) keeps a cursor in `state`.
The designs behind the contract are in [`design/imports.md`](../design/imports.md),
[`design/hooks.md`](../design/hooks.md), and [`design/exports.md`](../design/exports.md).

### Errors

Every function that can fail answers one of these:

| Error | Meaning | What to do |
|---|---|---|
| `not-granted` | Its permission was declined | Skip that work, or check `host::granted()` up front |
| `refused(why)` | Not done, and asking again won't help, such as a path outside the library | Count the item as failed and go on |
| `failed(why)` | Tried, and it didn't work, such as a file that doesn't exist | From `http.send`, stop and retry later. From `files`, it depends: a `Create` that fails because the file exists can usually be skipped. |
| `rate-limited(ms)` | `http.send` only: not sent, since its turn is `ms` away | Stop, and retry later |
| `timed-out` | `http.send` only: no reply in time | Stop, and retry later |

### Retries and failing

**To retry, return `Err` from `handle`** before recording the work as done. A hook's position
only moves on when a run succeeds, so the same batch comes again after a wait. The wait grows
with how long the plugin has been failing, from a minute to an hour. **Run now** is not
retried.

**72 hours of failing turns the plugin off** in that library, with the reason shown to admins.
Each hook keeps its own clock, which a success of that hook resets. A run for one user
(`played`, `playing`, `searched`) backs off the same way, but never turns the plugin off for
anyone else.

Treat a service's own `429` or `5xx` answer as a failure too. lrclib's
[`failure.rs`](lrclib-lyrics/src/failure.rs) shows which answers stop a run.

### Long work: a part per run

A run has 5 minutes, so work over a whole library never tries to finish in one. Do a part, save
your place in `state`, and carry on next run. lrclib's pass
([`src/plugin/pass.rs`](lrclib-lyrics/src/plugin/pass.rs)) is the pattern:

- **Save a cursor.** Read the library 500 tracks at a time, each page after the last ID of the
  one before. Save the last ID done in `state`, so a run cut short loses nothing. At the end of
  the library, delete it so the next run starts over.
- **Stop starting work in time.** lrclib stops starting lookups 120 s into a run. A lookup is at
  most two requests, each waiting up to 60 s for its turn and 15 s for a reply, so the last one
  finishes by 270 s, inside the 5 minutes. Size yours the same way.
- **Keep host calls down.** Every call costs a tick of compute time, so lrclib checks at most
  10,000 tracks a run.
- **Save a hook's leftover batch.** A `tracks-changed` batch there's no time for is saved in
  `state` and done first next run.
- **Ask to come back.** The `schedule` hook brings the next run.

## The manifest

A complete manifest, from [`lrclib-lyrics`](lrclib-lyrics/manifest.json):

```json
{
  "id": "lrclib-lyrics",
  "name": "LRCLIB Lyrics",
  "version": "0.1.0",
  "apiVersion": "0.1",
  "description": "Finds synced lyrics on lrclib.net for tracks that have none, and saves them beside the audio.",
  "author": "Jewelcase examples",
  "homepage": "https://lrclib.net",
  "settings": {
    "properties": {
      "syncedOnly": {
        "type": "boolean",
        "title": "Synced lyrics only",
        "description": "Skip tracks lrclib.net has only plain lyrics for, so every lyric shown follows along.",
        "default": false
      }
    }
  },
  "permissions": [
    { "permission": "libraryRead", "required": true, "reason": "To find tracks without lyrics." },
    {
      "permission": "network",
      "required": true,
      "reason": "To look up lyrics on lrclib.net, at most 45 times a minute.",
      "destinations": ["lrclib.net"],
      "rateLimits": { "lrclib.net": "45/min" }
    },
    { "permission": "libraryAdd", "required": false, "reason": "To save .lrc files beside your tracks." },
    { "permission": "tracksChanged", "required": false, "reason": "To look up lyrics for tracks as they are added." },
    { "permission": "schedule", "required": false, "reason": "To carry on through the library each hour, and look again a month later for tracks it saved nothing for.", "everyMinutes": 60 }
  ]
}
```

**The manifest is strict.** A key it doesn't know is refused at any level, so a misspelling
such as `rateLimit` is caught at install rather than silently ignored. The server is the only
validator, and lists every problem at once.

### Fields

| Field | Required | Rules |
|---|---|---|
| `id` | Yes | 2–64 lowercase letters, digits, and hyphens, starting with a letter or digit. Keep it the same across versions. |
| `name` | Yes | Shown to admins |
| `version` | Yes | Your plugin's version, such as `0.1.0` |
| `apiVersion` | Yes | The contract version it was built against: `"0.1"` |
| `description` | No | One line about what it does |
| `author` | No | |
| `homepage` | No | An `http` or `https` URL |
| `permissions` | Yes | What it asks for ([below](#permissions)). An empty list if nothing. |
| `settings` | No | What an admin can set ([below](#settings)) |
| `personalSettings` | No | What each user sets for themselves ([below](#settings)) |

### Permissions

Each entry in `permissions` asks for one permission or hook:

| Key | Rules |
|---|---|
| `permission` | Its name, from the tables below. Each may be asked for once. |
| `required` | `true` if the plugin can't work without it. An admin can decline the rest, and a plugin can't be enabled until its required permissions are granted. |
| `reason` | Why it needs it, shown to the admin |
| `destinations` | `network` only, and required there ([below](#network)) |
| `rateLimits` | `network` only, optional ([below](#network)) |
| `everyMinutes` | `schedule` only, and required there: how often it runs, at least 5 |

**What it can reach:**

| Permission | Allows | Granted |
|---|---|---|
| `libraryRead` | Reading tracks, albums, artists, and files | Per library |
| `libraryAdd` | Saving new files into the library | Per library |
| `libraryChange` | Replacing, renaming, and deleting files in the library | Per library |
| `network` | Requests to its `destinations` | For the whole server |
| `listeningActivity` | What connected users play | For the whole server |
| `searchActivity` | What users who share them search for | For the whole server |
| `hostRateLimits` | Rate limits that every plugin keeps to ([below](#network)) | For the whole server |

**When it runs (hooks):**

| Hook | Runs when | Also needs |
|---|---|---|
| `tracksChanged` | Tracks change | `libraryRead` |
| `albumsChanged` | Albums change | `libraryRead` |
| `artistsChanged` | Artists change | `libraryRead` |
| `scanFinished` | A scan finishes | `libraryRead` |
| `schedule` | Every `everyMinutes` | Nothing |
| `played` | A connected user finishes a play | `libraryRead`, `listeningActivity`, `personalSettings` |
| `playing` | A connected user starts a track | `libraryRead`, `listeningActivity`, `personalSettings` |
| `searched` | A user who shares searches settles on one | `libraryRead`, `searchActivity`, `personalSettings` |

**Connected users.** `played`, `playing`, and `searched` run only for users who connected the
plugin, which they do by saving its `personalSettings` on their account page. So a plugin with
those hooks must declare personal settings, such as an account token. `searched` also needs
each user to turn on sharing their searches.

### Network

`destinations` lists the host names a plugin may reach, such as `["lrclib.net"]`: not URLs,
ports, or patterns like `*.example.com`. An IPv6 address goes in brackets, and an international
domain in punycode. `["*"]` allows any public host, which admins see. A redirect to anywhere
else fails.

Loopback, link-local, and cloud metadata addresses can never be reached. Private network
addresses, such as `192.168.0.0/16`, are reached only through a destination that names the
host, never through `*`.

`rateLimits` paces requests to a destination, as a count per `s`, `min`, or `h`. A request
waits for its turn, or is refused as `rate-limited` if that's more than 60 s away. Each key is
one host from `destinations`, or any host when `destinations` is `["*"]`.

| Form | Counts |
|---|---|
| `"musicbrainz.org": "1/s"` | This plugin's requests |
| `"api.example.com": { "limit": "1/s", "per": "user" }` | Each connected user's requests apart, for a service that limits each account |
| `"musicbrainz.org": { "limit": "1/s", "per": "host" }` | Every plugin's requests to that host, for a service that limits the whole server. Needs `hostRateLimits`, and the host named in `destinations`. |

A `429` or `503` reply pauses your requests to that host for its `Retry-After`, or a short wait
that doubles each time, up to an hour, whether or not you declared `rateLimits`.

### Settings

`settings` are what an admin sets, per library or for the whole server. `personalSettings` are
what each user sets for themselves, such as their own account on a service. Both use the same
form, and their names must differ:

```json
"personalSettings": {
  "properties": {
    "token": {
      "type": "string",
      "title": "User token",
      "description": "From listenbrainz.org/settings.",
      "writeOnly": true
    }
  },
  "required": ["token"]
}
```

| Key | Rules |
|---|---|
| `properties` | One entry per setting. Names are up to 64 letters, digits, and `_`, starting with a letter. |
| `type` | `string`, `number`, `integer`, or `boolean` |
| `title`, `description` | Optional, shown beside the setting |
| `enum` | Optional: the only values allowed |
| `default` | Optional. Must match `type`, and be one of `enum` if there is one. |
| `writeOnly` | Optional: `true` for a secret, which is never shown again once saved |
| `required` | Optional: names that must be filled in when settings are saved |

The plugin reads them with `settings::get("name")`. A plugin can run before anyone has saved
its settings, so handle `None`. To check new values as they're saved, such as testing a token
against its service, handle `check-settings`: during it, `settings::get` answers with the values
being saved, and returning an error refuses them.

## Limits

Running out of compute or wall-clock time ends the run as a failure. The other limits shorten
or refuse the call.

| | Limit |
|---|---|
| Compute | 60,000 ticks a run, about 60 s of the plugin's own running time. Waiting costs nothing, but every host call costs a tick, so a run makes at most about 60,000 host calls. |
| Wall clock | 5 minutes, waiting included |
| Memory | 64 MiB, in one linear memory. A failed `memory.grow` returns -1. |
| Log | 1,000 lines a run, each cut to 500 characters |
| `state` | Keys up to 512 bytes, values up to 1 MiB |
| Library | 500 items per page or per list asked for by ID |
| Files | 500 entries per `entries` call, 8 MiB per read, 32 MiB per write |
| Network | 15 s for a reply (10 s to connect), 16 MiB bodies, 5 redirects |
| Hook batches | Up to 100 changes, plays, or searches per event |
| Plugin file | Up to 50 MB to install |

The exact figures live in [`crates/plugins/src/host.rs`](../crates/plugins/src/host.rs) and
beside it. How they're enforced is in
[`design/plugins.md` §7](../design/plugins.md#7-running-plugins).

**One memory only.** Rust through wit-bindgen builds one. A component composed from several
parts, as `wac` or `wasm-compose` make, has a memory for each and is refused at install.

**Rust is the recommended language.** JavaScript through componentize-js also works, but costs
far more: a 12 MB plugin against 72 KB, and about 20 times as long to read the library. That's
fine for events and background work, not for a whole-library pass someone is waiting on
([`design/plugins.md` §4](../design/plugins.md#4-guest-languages)).

## Testing

Keep the logic in plain Rust that doesn't touch the generated bindings, and only the glue in the
`#[cfg(target_arch = "wasm32")] mod plugin`. Then `cargo test` runs the logic on your machine.

- **Pass requests in as a function.** ListenBrainz's `listens::submit` takes the function that
  sends a batch, so its tests ([`src/listens/tests.rs`](listenbrainz/src/listens/tests.rs))
  can answer as a ListenBrainz that refuses or fails.
- **Lint both targets**, since the glue only compiles for WebAssembly:
  `cargo clippy --all-targets` and `cargo clippy --target wasm32-wasip2`.
- **Run it in a development server.** Install it, press **Run now**, and read the summary and
  log. A hook's run keeps only its summary, shown as **Last ran** under each library, so put
  what matters there.

In this repository, [`crates/plugins/tests`](../crates/plugins/tests/) also runs the packed
examples in the real host against a library in memory.

## Updating a plugin

To install a new version, uninstall the old one first; installing over it is refused. Its
settings come back, and so do its grants, for permissions it asks for again unchanged. Its
`state` and hook positions start over, users connect it again, and it comes back disabled until
an admin enables it.

When the contract changes, copy `crates/plugins/wit` again, update `apiVersion`, and rebuild.
