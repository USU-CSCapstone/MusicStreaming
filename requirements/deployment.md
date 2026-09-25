# Deployment Requirements

## Overview
Jewelcase is self-hosted software, which means the install is part of the product. A server nobody can stand up is not a fast server — it is nothing. [`general.md` §1](general.md#1-goals) promises deployment, upgrades, and backups approachable for a homelab rather than a professional operator; this file is what that promise means.

**Jewelcase ships as a container.** A user is given a `docker-compose.yml`, sets a few environment variables, and runs `docker compose up`. That is the whole supported path — not one option among several, but *the* way Jewelcase is installed, so there is one thing to document, one thing to test, and one thing that can go wrong.

---

## 1. What the User Provides

Three things, and nothing else:

- **A path to their music**, mounted into the container. Mounted **read-only**, because the core has no reason to write to it ([`general.md` §3.1](general.md#31-the-music-library-is-read-only-to-core)) and the filesystem should enforce that rather than trusting the code to.
- **A path for Jewelcase's own data** — index, playlists, queues, history, accounts, analysis results, transcode cache. This is the only thing Jewelcase writes, and the only thing that needs backing up ([§4](#4-backup--restore)).
- **A port** to reach it on.

Everything else has a working default. **A user who sets only these three runs a correct server**, and any variable they never touch is one the project chose well.

- **Configuration is environment variables**, not a config file to learn. Compose files are already how homelab users configure everything else they run.
- **Defaults are safe, not permissive.** No setting that ships enabled can weaken authentication ([`users.md` §3.1](users.md#31-protection)), expose the data directory, or grant write access to the music.
- **Bad configuration fails loudly at startup**, naming what is wrong and what was expected. A server must never start in a state that silently misbehaves later.

---

## 2. First Start

- **`docker compose up` is sufficient.** No init command, no migration to run by hand, no shell into the container.
- **The server is reachable as soon as it starts**, presenting the guided owner setup ([`users.md` §2.1](users.md#21-first-run)). It does not wait on a scan.
- **The first scan starts on its own** once a library is configured, and the library is usable while it runs ([`scanning.md` §5](scanning.md#5-behavior)).
- **A restart is never a fresh start.** Everything in the data directory survives the container being recreated, which is the normal way containers are updated.

---

## 3. Storage & Permissions

- **The data directory needs speed; the music does not** ([`performance.md` §1](performance.md#1-verification-hardware)). This is stated where users choose their mounts, because it is the single most consequential deployment decision and the easiest to get wrong.
- **Read-only music mounts are fully supported** ([`libraries.md` §2](libraries.md#2-storage)). A user who mounts their collection read-only loses only plugin-supplied enrichment.
- **File ownership is the user's to set**, through the conventional container mechanism, so files Jewelcase creates are readable by the person who owns the host.
- **Network storage is first-class** ([`libraries.md` §2](libraries.md#2-storage)). NFS and SMB mounts work, and where live filesystem watching is unreliable across them, scheduled scans cover it ([`scanning.md` §4](scanning.md#4-triggers)).

---

## 4. Backup & Restore

**Everything that matters is in one directory.** Backing up Jewelcase means copying the data directory; there is no second location, no database to dump separately, and no state hiding elsewhere.

- **The music is not Jewelcase's to back up.** It is the user's collection, backed up however they already back it up, and Jewelcase never claims responsibility for it.
- **A backup taken from a running server is valid.** Users do not stop their music server to protect it, so a copy taken live must restore cleanly rather than being subtly corrupt.
- **Restore is a copy back and a start.** Point a new container at a restored data directory and the server is exactly as it was — same accounts, playlists, history, statistics, and devices.
- **A restored server reconciles rather than re-indexing.** It picks up whatever changed on disk while it was gone ([`scanning.md` §7](scanning.md#7-change-handling)), at the cost of what changed, not the cost of the library.
- **What is safe to discard is documented.** Caches — transcodes, resized artwork — can be dropped from a backup and are rebuilt on demand. The user is told which parts are precious and which are merely convenient.

---

## 5. Upgrades

- **An upgrade is a new image tag.** Pull, recreate, done — the standard container workflow, with no upgrade procedure of its own.
- **Migrations run automatically at startup**, and the server does not begin serving until they complete. A half-migrated server is never reachable.
- **Migrations are safe to interrupt.** A power cut mid-upgrade leaves a server that starts correctly next time, not one that needs manual repair.
- **An upgrade never requires a full rescan.** Where a release genuinely needs to reprocess the library, it does so in the background and the library stays usable throughout ([`scanning.md` §5](scanning.md#5-behavior)).
- **Downgrades are documented, even where unsupported.** A user who upgrades into a problem needs to know before they start whether going back is possible, and restoring [§4](#4-backup--restore)'s backup is always the answer that works.
- **Breaking changes are announced in release notes**, never discovered at startup.

---

## 6. Remote Access

**Routing traffic to Jewelcase is the user's responsibility**, and the project says so plainly rather than implying more than it does. A homelab user already terminates TLS and routes hostnames for everything else they run, and Jewelcase does not duplicate a reverse proxy badly.

- **Jewelcase works correctly behind a reverse proxy** — correct client addresses for the per-origin sign-in limits ([`users.md` §3.1](users.md#31-protection)), working long-lived connections for realtime ([`realtime.md`](realtime.md)), and correct URLs in what it serves.
- **It never assumes it is at the root of a domain**, or that it is the only thing behind the proxy.
- **The documentation gives a working example** for the common proxies, so the promise in [`general.md` §1](general.md#1-goals) does not stop at the container boundary.
- **Jewelcase does not obtain certificates, manage DNS, or open ports.** These belong to the user's network, and a music server guessing at them is a music server breaking them.

---

## 7. Observability

What an operator needs to answer "is it healthy?" without reading logs.

- **A health endpoint** suitable for container orchestration, reporting whether the server is actually able to serve rather than merely running.
- **Logs are useful at default verbosity** — startup configuration, scan summaries, and errors — and never so noisy that real problems are buried.
- **Logs never contain credentials or tokens** ([`plugins.md` §6](plugins.md#6-configuration--credentials)).
- **The admin interface is the primary surface**, not the logs. Scan state, library health, current listeners, capacity, and plugin failures are all visible there ([`libraries.md` §3](libraries.md#3-management), [`performance.md` §7](performance.md#7-under-saturation), [`plugins.md` §11](plugins.md#11-isolation--boundaries)).
