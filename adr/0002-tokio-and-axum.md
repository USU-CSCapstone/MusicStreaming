# 0002. Tokio and Axum for the HTTP server

- **Date:** 2026-09-23 11:23 -0600
- **Status:** Accepted
- **Commit:** 52e600e

## Context
The server needs an async HTTP stack for the API, streaming, and WebSockets.

## Decision
- Tokio as the runtime and Axum as the HTTP framework, in one server process.
- Blocking work (scanning, tag reading, decoding) runs on dedicated OS threads, never on Tokio's workers.

## Alternatives considered
- **Why Axum.** Other frameworks don't have the same large ecosystem as Axum. It is maintained by the Tokio project and built on hyper and `tower`, so middleware such as `tower-http`'s compression, tracing, and static-file serving (needed for the SPA, 0003) comes off the shelf. It also has built-in WebSocket support.

## Consequences
Scanner and ffmpeg code expose synchronous APIs, and the server bridges them to async.
