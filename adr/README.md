# Architecture Decision Records

The high-level decisions behind Jewelcase — stack, key libraries, and architecture — and how they change over time. Detail belongs in [`design/`](../design/); this folder only records what was decided and when.

## Rules

- **One decision per file**, numbered in order: `NNNN-short-title.md`.
- **The date is when the decision landed in code**, not when it was first discussed.
- **Never rewrite a decision.** To change one, add a new ADR that says what it supersedes. The only edit an old ADR ever gets is its `Status` line, changed to `Superseded by NNNN`.
- **Status** is `Proposed` (on an unmerged branch), `Accepted`, or `Superseded by NNNN`.

## Template

```markdown
# NNNN. Title

- **Date:** YYYY-MM-DD HH:MM ±ZZZZ
- **Status:** Proposed | Accepted | Superseded by NNNN
- **Supersedes:** NNNN (if any)
- **Commit:** <hash>

## Context
Why a decision was needed, in a sentence or two.

## Decision
What was chosen.

## Alternatives considered
What else was looked at, and why it lost.

## Consequences
What it costs and what it commits us to.
```

## Index

| # | Decision | Date | Status |
|---|---|---|---|
| [0001](0001-rust-server-and-shared-core.md) | Rust for the server and a shared core | 2026-09-23 | Accepted |
| [0002](0002-tokio-and-axum.md) | Tokio and Axum for the HTTP server | 2026-09-23 | Accepted |
| [0003](0003-sveltekit-spa-web-client.md) | Svelte 5 and SvelteKit in SPA mode for the web client | 2026-09-23 | Accepted |
| [0004](0004-sqlite.md) | SQLite as the only database | 2026-09-25 | Accepted |
| [0005](0005-data-directory.md) | One data directory split into `state/` and `cache/` | 2026-09-25 | Accepted |
| [0006](0006-wasm-plugins-in-wasmtime.md) | Plugins as WebAssembly components in Wasmtime | 2026-09-30 | Proposed |
