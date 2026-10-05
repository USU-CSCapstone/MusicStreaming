# 0006. Plugins run as WebAssembly components in Wasmtime

- **Date:** 2026-09-30 10:20 -0600
- **Status:** Proposed
- **Commit:** (when it merges)

## Context
Plugins are how everything beyond serving a library gets built (`requirements/plugins.md`), and each must cost its own work and nothing else while reaching only what an admin grants.

## Decision
A plugin is one WebAssembly component carrying its manifest, hosted in-process by Wasmtime (`crates/plugins`) behind a WIT contract. The server keeps what is installed and granted in SQLite and each component under `state/`.

## Alternatives considered
- **Native child processes on a pipe.** Measured in the plugin spike ([`spikes/plugins` in `3364419`](https://github.com/USU-CSCapstone/MusicStreaming/tree/3364419344d92814817bc6b31f9b6e23071a222f/spikes/plugins), since removed): about 18× the call overhead, no portable memory limit, and no way to pause them for listeners (`design/plugins.md` §1).

## Consequences
- Wasmtime, WASI, and an HTTP client for the network permission join the server's build, which grows its compile time and binary.
- The plugin contract is a versioned WIT world that plugins build against, so changing it is an API change.
