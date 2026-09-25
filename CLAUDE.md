# Jewelcase

A self-hosted music streaming server and client suite. Users point it at music they already own; Jewelcase makes that collection feel like a commercial streaming service, running entirely on hardware they control. Open source, with a documented public API third parties build against.

The thesis: self-hosted music software is usually either *fast but crude* or *featureful but sluggish*. Jewelcase refuses that trade. Hundreds of thousands of tracks must feel exactly as instant as five hundred — online or offline, phone or desktop.

## Core principles

Load-bearing. Every requirement derives from them, and a proposal that violates one is rejected on that basis alone. Authoritative text is [`requirements/general.md` §3](requirements/general.md#3-core-principles).

| | |
|---|---|
| [§3.1](requirements/general.md#31-the-music-library-is-read-only-to-core) | **The music library is read-only to core.** All Jewelcase state lives in its own data directory. |
| [§3.2](requirements/general.md#32-the-scanner-is-the-only-ingestion-path) | **The scanner is the only ingestion path.** Plugins write, core reads; no side channel into the database. |
| [§3.3](requirements/general.md#33-the-host-is-the-limiter) | **The host is the limiter.** No caps on library size, listeners, or throughput. |
| [§3.4](requirements/general.md#34-optimistic-by-default) | **Optimistic by default.** Actions apply locally at once and confirm asynchronously. |
| [§3.5](requirements/general.md#35-offline-parity) | **Offline parity.** Only *what can play* and *what there is to search* may differ. |
| [§3.6](requirements/general.md#36-libraries-are-the-isolation-boundary) | **Libraries are the isolation boundary.** Leakage is a top-severity correctness bug. |
| [§3.7](requirements/general.md#37-payloads-carry-only-what-the-client-needs) | **Payloads carry only what the client needs.** Storage paths especially. |

## Constraints

- **Two verification machines**, both cheap and real: a Raspberry Pi 4 at 100k tracks / 10 transcoding listeners, and a Ryzen 7 2700X at 500k / 100. Every budget is met on **both**, and they are benchmarks, not caps ([`requirements/performance.md`](requirements/performance.md)).
- **Neither scale nor hardware may show in the interface.** Cheap hardware buys a smaller library, never a worse experience.
- **One adaptive interface** — responsive web (primary, installable as a PWA), Android, iOS via PWA. Every feature at every size, on touch, mouse, and keyboard.
- **The core serves a library and little else.** Anything beyond that is a plugin's to build, with no list of permitted capabilities ([`requirements/plugins.md`](requirements/plugins.md)).

## Out of scope

Non-music content, social features, acquiring music (a plugin concern), file management, casting protocols, and explicit taste signals in v1.

---

Requirements live in [`requirements/`](requirements/) — see [`requirements/CLAUDE.md`](requirements/CLAUDE.md) for the map and the conventions.
