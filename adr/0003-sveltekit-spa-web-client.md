# 0003. Svelte 5 and SvelteKit in SPA mode for the web client

- **Date:** 2026-09-23 11:27 -0600
- **Status:** Accepted
- **Commit:** cfc6c27

## Context
The web client is the primary client and must hold a one-frame input budget. It must also fit in a single container with no Node server.

## Decision
- Svelte 5, with runes on for all project code, and TypeScript.
- SvelteKit with `adapter-static` and an `index.html` fallback: static files served by the Rust server.
- Tooling: pnpm, Vitest (browser and Node), Playwright, ESLint, and oxfmt.

## Alternatives considered
- **Next.js (React).** Rejected. Next's main features (server rendering, server components, route handlers) need a Node server in production, which costs extra memory on low-power hardware. React is slower with its virtual DOM and with speed as a consideration, we chose Svelte.

## Consequences
- There are no SvelteKit server routes, so all data goes through the public API.
- The base path is fixed at build time and must be rewritten at startup.
