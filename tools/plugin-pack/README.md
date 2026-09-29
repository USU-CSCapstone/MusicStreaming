# plugin-pack

Turns a WebAssembly component into an installable Jewelcase plugin by embedding its
manifest (`design/plugins.md` §6). Node only, no dependencies.

```sh
node tools/plugin-pack/pack.mjs plugin.wasm manifest.json -o my-plugin.wasm
```

The output is what an admin uploads on the Plugins page or links to. Packing again replaces
the manifest rather than adding a second one, and a file that would not install is never
written. The error says what to fix.

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
    { "permission": "libraryWrite", "required": false, "reason": "To save .lrc files beside tracks." }
  ]
}
```

| Field | |
|---|---|
| `id` | Lowercase letters, digits, and hyphens. Stays the same across versions. |
| `apiVersion` | `0.1` |
| `permissions` | Any of `libraryRead`, `libraryWrite`, `network`, `listeningActivity` (`requirements/plugins.md` §4.1). Each needs a `reason`, which the admin sees. Mark as `required` only what the plugin cannot work without; an admin can decline the rest. |
| `destinations` | For `network` only: the host names it talks to, or `["*"]` for any. |

`manifest.mjs` is also what the server side uses to read and validate plugins on install.
