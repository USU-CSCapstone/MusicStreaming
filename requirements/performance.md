# Performance Requirements

## Overview
Every other requirements file says things must be *instant*, *immediate*, or *as fast at scale as at rest*. This file says what those words mean and how they are checked.

**Jewelcase is fast, and stays fast on modest hardware with a large library.** A low-power machine such as a Raspberry Pi is a first-class host, not a demo. The project deliberately sets goals rather than per-device numbers for now. Fixing figures to particular machines before the system exists would mean optimizing for a benchmark rather than for listeners.

The governing principle is **the host is the limiter** ([`general.md` §3.3](general.md#33-the-host-is-the-limiter)). Jewelcase imposes no ceilings of its own — not on library size, listeners, playlist length, or throughput. Where a limit exists it is a property of the hardware, and better hardware yields proportionally better results.

---

## 1. Target Hardware

**Low-power hardware is the design target.** A Raspberry Pi–class machine must serve a large library well, and every decision is made with it in mind. A design that only works on a desktop has missed the requirement.

- **Jewelcase's own data needs fast storage.** An SSD, not an SD card. Random I/O on the data directory decides how instant the interface feels, and no amount of software care makes up for slow storage.
- **The music itself may live anywhere** — NAS, network share, spinning disks. Only Jewelcase's data directory needs speed.
- **What binds differs by machine**, so measure before optimizing. On small hardware, memory and storage latency usually bind first. On larger hardware, the network often binds before the CPU, because direct high-resolution streams cost bandwidth rather than compute.

---

## 2. No Ceilings

**What the project measures and publishes describes what was measured, never what Jewelcase permits.**

- **Nothing counts listeners against a limit.** A machine with the capacity to serve 400 serves 400.
- **Nothing caps library size, playlist length, or context size.** Shuffling a context of hundreds of thousands of tracks is the same operation as shuffling ten.
- **Better hardware yields proportionally better results**, with no point at which the software becomes the constraint ([`general.md` §3.3](general.md#33-the-host-is-the-limiter)).
- **Concurrency is judged in the harsh case**, with every listener transcoding at once. Direct streaming is nearly free, so a goal that assumed it would prove nothing.

---

## 3. Interactions

- **Every input is acknowledged on the next frame.** This is what "no spinners" means in practice ([`general.md` §3.4](general.md#34-optimistic-by-default)), and it is the most important rule in this file.
- **Navigation, search while typing, sorting, filtering, and queue edits all feel instant**, at any library or context size.
- **Server confirmation is invisible.** The user has already moved on, so a slow confirmation shows up only when it fails.
- **Neither scale nor hardware shows.** A huge library feels the same as a small one. A 5,000-track playlist opens as fast as a 10-track one ([`conventions.md` §3](conventions.md#3-rendering)). A Pi feels the same as a desktop. Cheap hardware buys a smaller library, never a worse interface.

---

## 4. Playback

- **Pressing play produces sound without a perceptible wait**, transcoded or not.
- **Seeking and skipping are immediate**, including rapid skipping ([`playback.md` §2](playback.md#2-starting-seeking-and-buffering)).
- **Track-to-track transitions have no gap at all.** Gapless is exact ([`playback.md` §4](playback.md#4-transitions)).
- **A downloaded track starts faster than any stream**, with no network at all.

Under saturation, these hold for listeners who are already playing. New listeners are refused rather than served badly ([§7](#7-under-saturation)).

---

## 5. Scanning & Analysis

- **The first tracks are browsable within seconds** of a scan starting, and a newly copied-in album appears within seconds ([`scanning.md` §4](scanning.md#4-triggers)).
- **A full cold index of a large library finishes in a sitting**, not over days, even on low-power hardware.
- **Incremental cost is proportional to change**, never to library size. Ten changed files cost what ten files cost. This is the most important scanning property: a library that costs more to keep current as it grows will eventually stop being kept current.
- **Indexing is what users wait for; analysis is not.** Analysis touches every byte of every file, so it takes far longer than indexing. That is acceptable because a library is browsable and playable long before analysis finishes. Unanalyzed tracks play unnormalized ([`playback.md` §5](playback.md#5-loudness)) and are recommended on listening data alone ([`recommendations.md` §2.1](recommendations.md#21-how-the-music-sounds)).
- **Analysis is one pass, whatever it produces.** Decoding the audio dominates its cost, so loudness, sonic characteristics, and waveforms are all derived together ([`scanning.md` §1](scanning.md#1-supported-formats)). **The number of results analysis yields must never become a number of passes over the library.**
- **Analysis never competes with listening.** It runs at low priority, yields immediately under load, and can be interrupted ([`scanning.md` §5](scanning.md#5-behavior)).

---

## 6. Client Footprint

The client carries the whole catalog ([`offline.md` §1](offline.md#1-the-catalog-is-always-there)), so its cost is a requirement rather than an implementation detail.

- **The whole catalog, with its search index, costs less than a handful of songs.** That is a price no user would notice, for a library they can search offline in full.
- **Image stand-ins for the whole library cost about as much as one photograph.**
- **The full-resolution image cache is bounded, and visible to the user.**
- **The app starts quickly and stays light in memory**, on phones as well as desktops.
- **On-device search is as fast as server search** ([§3](#3-interactions)).
- **The on-device index supports the full matching rule, not a cheap subset.** Typo tolerance, diacritic folding, and word-order independence stay within the footprint rather than being traded away to fit it ([`search.md` §2.1](search.md#21-one-search-two-places-to-run-it)). An index that fits by matching worse has missed the requirement, not met it.
- **Downloaded lyrics are indexed for search**, and count against the download rather than the catalog ([`offline.md` §1.1](offline.md#11-footprint)).
- **Waveforms are not part of the catalog** ([`playback.md` §3](playback.md#3-the-waveform)). Per-track waveform data for a whole large library would cost more than the rest of the catalog combined, so it ships with downloads and on demand, never as a wholesale sync.
- **Sync is proportional to change**, never a periodic re-download of everything.
- **Lists are bounded by the viewport, not the data.** Rendering 500,000 rows costs the same as rendering 30.

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

Goals that are not measured are decoration.

- **Benchmarks are reproducible and public**, run against a generated large library so anyone can check the claims on their own hardware.
- **Low-power hardware is always among what is measured**, alongside more typical machines.
- **Percentiles, not averages.** Averages hide exactly the stutters users remember.
- **Regressions are defects.** A change that makes Jewelcase noticeably slower is not a tradeoff to accept quietly.
- **The published numbers are the real ones.** Where a goal is missed, it is documented as missed rather than restated to fit.
