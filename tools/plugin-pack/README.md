# plugin-pack

Turns a WebAssembly component into an installable Jewelcase plugin by embedding its
manifest (`design/plugins.md` §6). Node only, no dependencies.

```sh
node tools/plugin-pack/pack.mjs plugin.wasm manifest.json -o my-plugin.wasm
```

The output is what an admin uploads on the Plugins page or links to. Packing again replaces
the manifest rather than adding a second one, and a file that would not install is never
written. The error says what to fix.

## The component

A plugin is one component with **one linear memory**, which may grow to 64 MiB. Rust through
wit-bindgen for `wasm32-wasip2`, and JavaScript through componentize-js, each build one. A
component composed from several, as wac or wasm-compose make, brings a memory for each part and
is refused at install. The other caps a run keeps to are in `design/plugins.md` §7.

## The manifest

```json
{
  "id": "lrclib-lyrics",
  "name": "LRCLIB Lyrics",
  "version": "0.1.0",
  "apiVersion": "0.1",
  "description": "Fetches synced lyrics for tracks that have none.",
  "author": "You",
  "permissions": [
    { "permission": "libraryRead", "required": true, "reason": "To find tracks without lyrics." },
    { "permission": "network", "required": true, "reason": "To fetch lyrics.", "destinations": ["lrclib.net"] },
    { "permission": "libraryAdd", "required": false, "reason": "To save .lrc files beside tracks." }
  ]
}
```

| Field | |
|---|---|
| `id` | Lowercase letters, digits, and hyphens. Stays the same across versions. |
| `apiVersion` | `"0.1"`, the contract ([`crates/plugins/wit/plugin.wit`](../../crates/plugins/wit/plugin.wit)). |
| `permissions` | Any of `libraryRead`, `libraryAdd`, `libraryChange`, `network`, `listeningActivity`, `searchActivity`, `hostRateLimits` (`requirements/plugins.md` §4.1); and the hooks `tracksChanged`, `scanFinished`, `schedule`, `played`, `playing`, `searched`, `albumsChanged`, and `artistsChanged` (`design/plugins.md` §7, `design/hooks.md`). Each needs a `reason`, which the admin sees. Mark as `required` only what the plugin cannot work without; an admin can decline the rest. |
| `destinations` | For `network` only: the host names it talks to, such as `lrclib.net` (not a URL, a port, or a pattern; an IPv6 address in brackets, an IDN in punycode), or `["*"]` for any. |
| `rateLimits` | For `network` only, optional: the most often each destination may be asked, as `{ "musicbrainz.org": "1/s" }`, per `s`, `min`, or `h`. Each key is one host name from `destinations`, or any host if they are `["*"]`; no wildcards. `"1/s"` counts only this plugin's requests; `{ "limit": "1/s", "per": "user" }` counts its runs for each connected user apart, for a service that limits each account; `"per": "host"` is kept to by every plugin's requests to that host, for one that limits the whole server; it needs `hostRateLimits` and a host named in `destinations`, not reached through `*` (`design/plugins.md` §7). |
| `everyMinutes` | For `schedule` only: how often it runs, at least 5. |
| `settings` | Optional. What an admin can set, as `{ "properties": { "name": { "type": "string" \| "number" \| "integer" \| "boolean", "title", "description", "enum", "default", "writeOnly" } }, "required": [names] }`, where a `default` is one of the `enum`, if it has one. `writeOnly` marks a secret, never shown again once entered. The plugin reads them with `settings.get`, and checks new ones on `check-settings`. |
| `personalSettings` | Optional. What each user sets for themselves, such as their own account on a service, in the same form as `settings` and with names of its own. Users who save them have connected the plugin, and only their listening reaches it. A run sees both under one set of names. |

No other key is allowed, at any level: a misspelt one, such as `rateLimit`, is refused rather
than ignored.

The server validates on install with its own reader in [`crates/plugins`](../../crates/plugins/),
which is the authority. `manifest.mjs` validates the same way, for authors, the web mock, and its
tests, and both are held to [`manifest-cases.json`](../../crates/plugins/manifest-cases.json).
Change one, and the cases, together: add a case as `{ "name", "manifest" }` and run
`node tools/plugin-pack/cases.mjs` to record what this module says about each. A case's manifest
leaves out `apiVersion`, which each test adds, unless the case is about it.
