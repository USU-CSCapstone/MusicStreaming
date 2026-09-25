# Performance Requirements

## Overview
Every other requirements file says things must be *instant*, *immediate*, or *as fast at scale as at rest*. This file replaces those adjectives with numbers, and says how they are verified.

The governing principle is **the host is the limiter** ([`general.md` §3.3](general.md#33-the-host-is-the-limiter)). Jewelcase imposes no ceilings of its own — not on library size, listeners, playlist length, or throughput. Where a limit exists it is a property of the hardware, and better hardware yields proportionally better results.

---

## 1. Verification Hardware

Two specific machines, both cheap, both real. Every budget below is met on **both** — a Raspberry Pi must feel like the desktop, at its own scale.

| | Machine | Library | Listeners, all transcoding |
|---|---|---|---|
| **Baseline** | Raspberry Pi 4 — 4 cores, 4 GB+ | **100,000 tracks** | **10** |
| **Full** | Ryzen 7 2700X, 16 GB DDR4 | **500,000 tracks** | **100** |

Conditions that are part of the target, not excuses for missing it:

- **Jewelcase's own data lives on an SSD.** On the Pi that means USB 3.0, not microSD — microSD's random I/O cannot hold the [§3](#3-interaction-budgets) budgets, and no amount of software care compensates.
- **The music itself may live anywhere** — NAS, network share, spinning disks. Only Jewelcase's data directory needs speed.
- **4 GB is the floor on the Pi.** A 100,000-track index, its search index, and ten transcodes do not fit in 2 GB.
- **The Pi needs cooling.** Sustained transcoding and analysis will throttle a bare board. That is a property of the hardware, not a Jewelcase limit.

**What actually binds is different on each machine**, and worth knowing before optimizing the wrong thing:

- **On the Pi, transcoding is not the constraint.** Ten concurrent transcodes is well under one of its four cores. Memory and storage latency are what bind.
- **On the desktop, the network binds before the CPU does.** A hundred transcodes costs roughly two cores of eight, while a hundred *direct* high-resolution streams is around 450 Mbps — half a gigabit link.

---

## 2. No Ceilings

The figures in [§1](#1-verification-hardware) are **what the project verifies and publishes, not what Jewelcase permits.**

- **Nothing counts listeners against a limit.** A machine with the capacity to serve 400 serves 400.
- **Nothing caps library size, playlist length, or context size.** A 500,000-track shuffle and a 10-track shuffle are the same operation.
- **Better hardware yields proportionally better results**, with no point at which the software becomes the constraint ([`general.md` §3.3](general.md#33-the-host-is-the-limiter)).

**The transcoding figures are deliberately the harsh case** — every listener transcoding at once, not a realistic mix. Direct streaming is nearly free, so a target that assumed it would verify nothing.

---

## 3. Interaction Budgets

Measured at the **95th percentile**, on **both** machines at their own library size ([§1](#1-verification-hardware)). Averages are excluded on purpose — they hide exactly the stutters users remember.

| Interaction | Budget |
|---|---|
| Visible response to any input | **1 frame (~16 ms)** |
| Navigate to a fully rendered view | **100 ms** |
| Search results while typing | **50 ms** |
| Sort, filter, or browse | **100 ms** |
| Any queue edit, at any context size | **50 ms** |
| Server confirmation of an optimistic action | **300 ms** |

- **The first row is the important one.** Every user action is acknowledged on the next frame, because that is what "no spinners" means in practice ([`general.md` §3.4](general.md#34-optimistic-by-default)).
- **Server confirmation is invisible.** The user has already moved on; a slow confirmation shows up only when it fails.
- **Neither scale nor hardware may appear in these numbers.** A 500,000-track library measures the same as a 500-track one, a 5,000-track playlist opens as fast as a 10-track one ([`conventions.md` §3](conventions.md#3-rendering)), and a Pi measures the same as a desktop. Cheap hardware buys a smaller library, never a worse interface.

---

## 4. Playback Budgets

| Event | Budget |
|---|---|
| Press play → sound, direct stream | **500 ms** |
| Press play → sound, transcoded | **1 s** |
| Seek → sound | **300 ms** |
| Skip → sound | **500 ms** |
| Track-to-track transition | **0 ms** — gapless is exact ([`playback.md` §4](playback.md#4-transitions)) |
| Playing a downloaded track | **Faster than any of the above**, with no network at all |

Under saturation these hold for listeners already playing. New listeners are refused rather than served badly ([§7](#7-under-saturation)).

---

## 5. Scanning & Analysis Budgets

Storage speed dominates indexing, so these assume the library is on reasonable local disks. Both machines land in the same place, the Pi because it is slower and the desktop because its library is five times larger.

| Work | Pi 4, 100k tracks | Desktop, 500k tracks |
|---|---|---|
| First tracks browsable after a scan starts | **Seconds** | **Seconds** |
| Full library indexed from cold | **Under 1 hour** | **Under 1 hour** |
| Incremental scan, ten changed files | **Under 1 second** | **Under 1 second** |
| A newly copied-in album appearing | **Seconds** ([`scanning.md` §4](scanning.md#4-triggers)) | **Seconds** |
| Audio analysis, whole library | **2–3 days** | **2–3 days** |

- **Indexing is what users wait for; analysis is not.** A library is browsable and playable long before analysis finishes. Unanalyzed tracks play unnormalized ([`playback.md` §5](playback.md#5-loudness)) and are recommended on listening data alone ([`recommendations.md` §2.1](recommendations.md#21-how-the-music-sounds)).
- **Analysis is one pass, whatever it produces.** Decoding the audio dominates its cost, so loudness, sonic characteristics, and waveforms are all derived together ([`scanning.md` §1](scanning.md#1-supported-formats)). Days, not hours, is the honest figure — this is the only work in Jewelcase that must touch every byte of every file, and **the count of results it yields must never become a count of passes over the library.**
- **Analysis never competes with listening.** It runs at low priority, yields immediately under load, and is interruptible ([`scanning.md` §5](scanning.md#5-behavior)).
- **Incremental cost is proportional to change**, never to library size. This is the single most important scanning property: a library that costs more to keep current as it grows will eventually stop being kept current.

---

## 6. Client Footprint

The client carries the whole catalog ([`offline.md` §1](offline.md#1-the-catalog-is-always-there)), so its cost is a requirement rather than an implementation detail.

| | Budget, at 500,000 tracks |
|---|---|
| Catalog and search index on device | **Under 150 MB** |
| Image stand-ins, whole library | **Under 8 MB** |
| Full-resolution image cache | **Bounded, and visible to the user** |
| Cold start to interactive | **Under 2 s** |
| Memory in use, mobile | **Under 250 MB** |
| On-device search, full catalog | **Under 50 ms** — the server's budget ([§3](#3-interaction-budgets)) |

- **The whole catalog must cost less than a handful of songs.** At roughly 7 MB for one track at high quality, a 150 MB budget is about twenty — a price no user would notice, for a library they can search offline in full.
- **The on-device index must support the full matching rule, not a cheap subset.** Typo tolerance, diacritic folding, and word-order independence are inside this budget, not traded away to fit it ([`search.md` §2.1](search.md#21-one-search-two-places-to-run-it)). An index that fits by matching worse has missed the requirement, not met it.
- **Downloaded lyrics are indexed for search** and counted against the download, not this budget ([`offline.md` §1.1](offline.md#11-footprint)).
- **Waveforms are not part of the catalog** ([`playback.md` §3](playback.md#3-the-waveform)). Per-track waveform data at 500,000 tracks would exceed this entire budget on its own, so it ships with downloads and on demand, never as a wholesale sync.
- **Sync is proportional to change**, never a periodic re-download of everything.
- **Lists are bounded by the viewport, not the data.** Rendering cost is the same for 500,000 rows as for 30.

---

## 7. Under Saturation

A saturated host degrades **predictably and legibly**, never unpredictably.

- **Listeners already playing are protected.** Their quality does not drop and their streams do not break because someone else pressed play.
- **New streams are refused, with an explanation.** A listener who cannot start is told the server is at capacity — not left with silence, an error code, or degraded audio they did not choose.
- **Background work yields first.** Scanning, analysis, and image processing surrender resources to playback before any listener is affected ([`scanning.md` §5](scanning.md#5-behavior)).
- **Admins can see it happening** — current listeners, transcodes in flight, and what capacity is being hit — so the answer to "why was I refused" is available rather than guessed at.
- **Refusal is a hardware signal.** Hitting capacity means the machine is full, and adding hardware raises the ceiling proportionally. It never means Jewelcase decided a number was enough.

---

## 8. Measurement

Budgets that are not measured are decoration.

- **Benchmarks are reproducible and public**, run against a generated library at full scale so anyone can verify the claims on their own hardware.
- **Both machines are measured** for every budget, and results are published per machine. Naming specific, buyable hardware means the claims can be checked rather than believed.
- **Percentiles, not averages.** p95 is the budget; p99 is reported alongside it.
- **Regressions block releases.** A change that misses a budget is a defect, not a tradeoff to be accepted quietly.
- **The published numbers are the real ones.** Where a target is missed, it is documented as missed rather than restated to fit.
