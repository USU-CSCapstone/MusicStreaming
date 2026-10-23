# Deployment Requirements

## Overview
Jewelcase is self-hosted software, which means the install is part of the product. A server nobody can stand up is not a fast server — it is nothing. [`general.md` §1](general.md#1-goals) promises deployment, upgrades, and backups approachable for a homelab rather than a professional operator; this file is what that promise means.

**Jewelcase ships as a container.** A user is given a `docker-compose.yml`, sets a few environment variables, and runs `docker compose up`. That is the whole supported path — not one option among several, but *the* way Jewelcase is installed, so there is one thing to document, one thing to test, and one thing that can go wrong.

---

## 1. What the User Provides

Three things, and nothing else:

- **A path to their music**, mounted into the container. **Writable or read-only, as the user chooses.** The core only ever reads it ([`general.md` §3.1](general.md#31-the-music-library-is-read-only-to-core)), so it works completely on a read-only mount. A writable mount is what lets plugins granted write access save into the library ([`plugins.md` §3](plugins.md#3-writing-to-the-library)), so it is the default.
- **A path for Jewelcase's own data** — index, playlists, queues, history, accounts, analysis results, transcode cache. This is the only thing Jewelcase writes, and the only thing that needs backing up ([§4](#4-backup--restore)).
- **A port** to reach it on.

Everything else has a working default. **A user who sets only these three runs a correct server**, and any variable they never touch is one the project chose well.

- **Configuration is environment variables**, not a config file to learn. Compose files are already how homelab users configure everything else they run.
- **Defaults are safe, not permissive.** No setting that ships enabled can weaken authentication ([`users.md` §3.1](users.md#31-protection)), expose the data directory, or give a plugin write access to the music — a writable mount only makes that grant possible ([`plugins.md` §4](plugins.md#4-trust--permissions)).
- **Bad configuration fails loudly at startup**, naming what is wrong and what was expected. A server must never start in a state that silently misbehaves later.

---

## 2. First Start

- **`docker compose up` is sufficient.** No init command, no migration to run by hand, no shell into the container.
- **The server is reachable as soon as it starts**, presenting the guided owner setup ([`users.md` §2.1](users.md#21-first-run)). It does not wait on a scan.
- **Setup is completed right after first start.** Until it is, anyone who reaches the server can claim it as owner, including a web page open in a browser on the same network, through DNS rebinding, so the documentation tells the user to finish setup before doing anything else.
- **The first scan starts on its own** once a library is configured, and the library is usable while it runs ([`scanning.md` §5](scanning.md#5-behavior)).
- **A restart is never a fresh start.** Everything in the data directory survives the container being recreated, which is the normal way containers are updated.

---

## 3. Storage & Permissions

- **The data directory needs speed; the music does not** ([`performance.md` §1](performance.md#1-target-hardware)). This is stated where users choose their mounts, because it is the single most consequential deployment decision and the easiest to get wrong.
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
- **Exposure to the internet goes through a reverse proxy.** Jewelcase does not cap connections itself ([`general.md` §3.3](general.md#33-the-host-is-the-limiter)), so it relies on the proxy in front to absorb clients that hold many connections open by reading slowly, and the documentation says so.
- **Only a proxy the user names is believed** about what it forwards: the client's address, the host the client asked for, and whether it came over HTTPS. Any other peer is taken as the client itself, whatever its headers claim, or anyone could pose as any client. `JEWELCASE_TRUSTED_PROXIES` lists those proxies as addresses or networks (`172.16.0.0/12`), and is loopback alone unless set; a proxy left out makes all its clients one client to the sign-in limits. A proxy in another container is named by its address on the network it shares with Jewelcase, and one on the host by that network's gateway.
- **Plugins are not granted destinations inside a trusted proxy's range.** A plugin's requests leave from the server, so one that can reach Jewelcase from there could claim any client address in `X-Forwarded-For`, and the documentation warns admins of this.
- **A session begun over HTTPS stays on HTTPS.** When a trusted proxy says the client came over HTTPS, the session cookie is marked `Secure`, so one plain-HTTP request to the same host never carries it.
- **Docker's IPv6 proxy is explained.** On a host with IPv6, a published port whose container network has none relays every IPv6 client through Docker's own proxy, from the network's gateway. Those clients then share one address for the sign-in limits, and if the gateway is trusted, any of them can claim to be anyone. The documentation gives the two ways out: publish the port on IPv4 only (`0.0.0.0:8080:8080`), or publish no port and reach Jewelcase only through a proxy on its network.
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
