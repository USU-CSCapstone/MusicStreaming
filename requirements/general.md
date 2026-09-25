# General Requirements

## Overview
Jewelcase is a self-hosted music streaming server and client suite. Users point it at the music they already own, and Jewelcase turns that collection into a fast, modern listening experience across every device they use — with the responsiveness people expect from commercial streaming services, but running entirely on hardware they control.

The project's guiding tension: self-hosted music software is usually either *fast but crude* or *featureful but sluggish*. Jewelcase refuses that trade. A library of hundreds of thousands of tracks must feel exactly as instant as a library of five hundred, online or offline, on a phone or a desktop.

Jewelcase is open source, and its server exposes a documented public API that third-party developers are free to build against.

This file holds only what the rest of the set derives from: the goals, the boundaries, the principles, and the platforms. Everything else has its own file ([`CLAUDE.md`](CLAUDE.md)).

---

## 1. Goals

- **Own your library, keep the modern experience** — instant search, a real queue, offline playback, cross-device handoff, listening stats, discovery.
- **Feel instant, always.** No spinners on interactions the user initiates (figuratively).
- **Never touch the user's files.** Uninstall Jewelcase and find the collection exactly as it was.
- **Scale to the host, not to an arbitrary ceiling.** The limiting factor on performance must be the server's resources: CPU, disk, and bandwidth.
- **Be genuinely extensible.** The core serves a library and little else. Anything beyond that is a plugin's to build, with no list of permitted capabilities and no need to wait for a core release ([`plugins.md`](plugins.md)).
- **Work offline as a first-class mode**, not a degraded fallback ([`offline.md`](offline.md)).
- **Be pleasant to self-host.** Deployment, upgrades, and backups approachable for a homelab, not just a professional operator ([`deployment.md`](deployment.md)).

The rules these translate into are [§3](#3-core-principles).

---

## 2. Non-Goals

Explicitly out of scope for the Jewelcase server. Naming them protects the scope of the core product.

- **Social features.** No follows, feeds, comments, or public profiles.
- **Being a general-purpose file manager.** No file browsing, moving, or deletion tools.

Narrower exclusions are stated where they bite: casting protocols in [`playback.md` §9](playback.md#9-output-scope), explicit taste signals in [`analytics.md` §7](analytics.md#7-no-explicit-signals-in-v1), user-applied tags in [`tags.md` §1](tags.md#1-tags-come-from-files).

---

## 3. Core Principles

These are load-bearing. Requirements throughout the other files derive from them, and a proposed design that violates one should be rejected on that basis alone.

### 3.1 The Music Library Is Read-Only to Core
The server **only ever reads** from the music library — never writing audio, tags, artwork, or lyrics into that tree. All Jewelcase-owned state (index, playlists, queues, history, accounts, transcode caches) lives in its own data directory.

The one qualification is plugins, which write to the library without restriction ([§3.2](#32-the-scanner-is-the-only-ingestion-path), [`plugins.md` §3](plugins.md#3-writing-to-the-library)). The core's promise is unchanged: uninstall Jewelcase with no plugins installed and the collection is exactly as it was.

### 3.2 The Scanner Is the Only Ingestion Path
Content enters exactly one way: it appears in the library and the scanner finds it — whether copied in by a user or written by a plugin.

**Plugins are writers; the core is a reader.** A plugin-fetched biography is ingested exactly as one placed by hand; the two are indistinguishable. There is no privileged plugin-to-database side channel, no second pipeline, and no ordering ambiguity between "user data" and "plugin data".

Music a plugin merely *surfaces* rather than places is not library content and is never ingested at all ([`plugins.md` §10.1](plugins.md#101-it-is-not-library-content)).

### 3.3 The Host Is the Limiter
No fixed caps on library size, concurrent listeners, or throughput. Where a limit exists it is a direct function of CPU, memory, disk, or bandwidth, and adding hardware yields proportionally better performance.

The figures the project verifies and publishes are benchmarks, not ceilings ([`performance.md` §1](performance.md#1-verification-hardware)–[§2](performance.md#2-no-ceilings)).

### 3.4 Optimistic by Default
User actions apply to the local UI immediately and confirm asynchronously. Where the server disagrees, the client reconciles and — if the reconciliation is user-visible — explains what happened.

### 3.5 Offline Parity
Client behavior is deterministic and identical with or without a network. Stronger than "offline mode works": the same inputs produce the same queue order, the same sort, and the same navigation on either side of the connectivity boundary.

Two differences are permissible, and only these:

1. **Whether a track's audio can actually play.**
2. **What a device has to search.** Search offline covers the catalog the device holds and the lyrics it has downloaded — a smaller *corpus*, never a smaller *capability* ([`search.md` §2](search.md#2-where-search-runs)).

The second is a difference of input, not of quality. Client-side search matches, ranks, and presents exactly as the server does, and **where a device holds the whole library and its lyrics, offline results are identical to the server's** — not similar, identical. Any divergence beyond missing input is a defect.

### 3.6 Libraries Are the Isolation Boundary
Every piece of content and metadata is scoped to a library, enforced on every request. Cross-library leakage is a correctness bug of the highest severity.

### 3.7 Payloads Carry Only What the Client Needs
Responses are trimmed to what a client can act on or render. Internal bookkeeping — how a shuffled order was derived, played-track state, scan internals — is never sent to a client that does not need it to render.

Storage paths are the case worth naming: a path carries no meaning in Jewelcase ([`scanning.md` §2](scanning.md#2-tags-are-the-only-truth)) and discloses the shape of the host's filesystem, so it is exposed deliberately and narrowly rather than by default ([`tracks.md` §4](tracks.md#4-audio-properties)).

---

## 4. Target Platforms

| Platform | Status | Notes |
|---|---|---|
| **Responsive web app** | Primary | One adaptive UI from phone through desktop, installable as a PWA. The reference client and the baseline for every feature. |
| **Android app** | In scope | Background audio, media-session and lock-screen controls, durable offline downloads. |
| **iOS** | PWA only | Served by the installable web app. No native client in v1. |
| **Third-party clients** | Community | Not built or maintained by the project, but enabled — the public API is documented and stable enough to build against. |

**One adaptive interface covers all of them.** Every feature works at every size and on every input — touch, mouse, and keyboard — and no platform gets a reduced version of the product.

**The public API is a commitment, not a byproduct.** It is what third-party clients build against and what defines the reach of a plugin ([`plugins.md` §1](plugins.md#1-an-open-surface)).
