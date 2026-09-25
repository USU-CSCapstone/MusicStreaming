# Playback Requirements

## Overview
Playback is the point of the system. Scanning, queues, and search all exist to get audio to a listener's ears without friction.

This file covers how audio is delivered, how it sounds, how a listener navigates within a track, and how playing behaves. **What** plays next is [`queue.md`](queue.md); **whether** a local copy is used instead of a stream is [`offline.md` §4](offline.md#4-choosing-between-local-and-stream).

---

## 1. Delivery

- **Direct streaming is the default.** Original bytes whenever the client can play them — full quality at near-zero server cost.
- **Transcoding happens only when it must**: the client cannot decode the format, or the applicable quality setting — the connection's for a stream, the device's download quality for a download ([`users.md` §6](users.md#6-profile--preferences)) — is below the source. Never otherwise.
- **Quality is automatic, not managed.** It follows the per-connection settings ([`users.md` §6](users.md#6-profile--preferences)).
- **The user can see what they are hearing** — original or transcoded, and at what bitrate. Automatic must not mean opaque, particularly for people who chose self-hosting to keep their lossless files lossless.
- **Repeat listening costs no more than the first time.** Hearing a track again at the same quality does not repeat work already done.
- **Both paths reach the concurrency targets** for each verification machine, with every listener transcoding ([`performance.md` §1](performance.md#1-verification-hardware)).

---

## 2. Starting, Seeking, and Buffering

- **Playback starts immediately.** Pressing play produces sound without a perceptible wait, on any track in the library.
- **Seeking is instant and unrestricted**, anywhere in a track, including within a transcoded stream. A transcode is not an excuse for a track that cannot be scrubbed. Where the user seeks *to* is the waveform's job ([§3](#3-the-waveform)).
- **Transitions never stall.** The next track is ready before the current one ends, so there's no gap in playback.
- **A stalled connection recovers rather than restarts.** Playback resumes from where it stopped, never from the beginning of the track.
- **Skipping is instant**, including rapid skipping through several tracks, which must not queue up audio the user has already skipped past.

---

## 3. The Waveform

**The progress bar is a waveform of the track playing.** Its purpose is navigational: a listener looking for the second chorus or the quiet passage before it can see where that is and scrub to it, rather than guessing repeatedly at a featureless line.

- **Legible on any material.** Structure is visually distinguishable in a heavily-compressed pop master, a dynamic orchestral recording, and an ambient piece alike, each scaled to its own dynamics. A waveform that renders as a solid block or a flat line has failed — and a naive amplitude plot produces exactly those two failures on exactly that material, which is why this is a requirement rather than an assumption.
- **Honest.** Shaping for legibility must never invent structure or move a transition from where it falls. Scrubbing to what looks like the drop lands on the drop.
- **Deterministic.** The same audio always yields the same waveform, on every device and between sessions.
- **Produced by the single analysis decode pass** ([`scanning.md` §1](scanning.md#1-supported-formats)) — never a second pass over the library — from the original file, so a track looks identical streamed, transcoded, or downloaded, and unchanged by loudness normalization ([§5](#5-loudness)).
- **Absent until analyzed.** Analysis takes days at full scale ([`performance.md` §5](performance.md#5-scanning--analysis-budgets)), so a track without one plays immediately behind an ordinary progress bar. That plain bar is a legitimate state: no placeholder shape and no fake waveform suggesting structure nobody measured.
- **Scrubbable everywhere.** Dragging it seeks within the normal budget ([`performance.md` §4](performance.md#4-playback-budgets)), at every size and by touch, mouse, and keyboard, with the keyboard path never depending on seeing the shape ([`general.md` §4](general.md#4-target-platforms)).
- **Carried like lyrics, not like catalog** — with downloads and on demand, never pre-synced wholesale ([`offline.md` §1.1](offline.md#11-footprint)).

---

## 4. Transitions

- **Gapless playback is always on**, and is exact — continuous albums, live recordings, and DJ mixes play without a seam, including across a transcode.
- **Crossfade is optional**, off by default, with a user-set duration ([`users.md` §6](users.md#6-profile--preferences)).
- **Gapless wins where the two conflict.** Crossfade is suppressed between tracks meant to run continuously, because fading across a deliberate transition damages the recording.
- **Neither applies to manual skips.** Crossfade shapes automatic transitions; a user pressing next gets the next track immediately.
- **Silence skipping is optional** ([`users.md` §6](users.md#6-profile--preferences)), trimming long silences and trailing dead air without ever clipping the start or end of actual music.

---

## 5. Loudness

**Jewelcase measures loudness itself, during scanning** ([`scanning.md` §1](scanning.md#1-supported-formats)), rather than trusting whatever the files were tagged with — which is what makes a library assembled from many sources normalize consistently.

- **Normalization is a user preference**: off, per-track, or per-album. Album mode preserves the relative loudness within a release; track mode evens out a shuffled mix.
- **Unmeasured tracks play immediately**, unnormalized, and normalize once measured. Analysis never gates listening.
- **Normalization never clips.** Raising a quiet track must not distort it.

---

## 6. Playback State

- **State is the account's, not the device's.** What is playing, where in the track, and whether it is paused are the same everywhere the user is signed in.
- **Exactly one device plays at a time**, and moving playback between devices keeps the position and the queue. Takeover and handoff are specified in [`realtime.md` §2](realtime.md#2-one-player-at-a-time)–3.
- **Playback resumes where it was left**, at the right track and the right position, after closing the app, restarting the device, or signing in elsewhere.
- **Position updates are frequent enough to be trustworthy** — a device picked up after a handoff is never seconds behind.

---

## 7. System Integration

- **Audio continues in the background**, with the app closed or the screen off, on every platform ([`general.md` §4](general.md#4-target-platforms)).
- **System controls work** — lock screen, notification shade, media keys, headset buttons, car and watch controls — showing the correct track, artwork, and position.
- **Interruptions are handled properly.** Playback pauses for calls and alarms, ducks for short system audio, and resumes afterward.
- **Unplugging pauses.** Disconnecting headphones or Bluetooth stops playback rather than broadcasting it to a room.

---

## 8. When Playback Fails

**A track that cannot play and a connection that is not working look identical for a moment, and call for opposite responses.** Treating the second as the first destroys a listening session.

- **Transient failures are retried before skipping.** A track that fails to load is attempted a few times, spaced far enough apart for a connection to actually recover, before playback gives up on it.
- **Failures never cascade.** Several tracks failing in a row means the problem is the connection, not the music. Playback **stops and says so**, holding its place — rather than failing twenty requests and burning through the whole queue in two seconds.
- **Permanent failures skip immediately.** A file that is gone, unreadable, or in a format the client cannot decode is not worth retrying.
- **Missing and undownloaded tracks are skipped silently** — expected states, not failures ([`conventions.md` §6](conventions.md#6-availability)).
- **A real failure is explained once, plainly**, without a dialogue interrupting the session.
- **Position and order always survive.** A halted queue resumes exactly where it stopped once the connection returns, with nothing lost and nothing silently skipped past.
- **A server that becomes unreachable falls back to downloaded audio** and keeps playing ([`offline.md` §4](offline.md#4-choosing-between-local-and-stream)).
- **Repeated failures are reported to admins** as a server problem, rather than left as an inexplicable listening experience.

---

## 9. Output Scope

**Jewelcase plays to Jewelcase clients.** Playback moves between the user's own signed-in devices ([§6](#6-playback-state)), and anything beyond that — a Bluetooth speaker, a system-level AirPlay target — is reached by whatever the host device already does.

Casting protocols — Chromecast, AirPlay, DLNA — are **out of scope**. Supporting them means owning a matrix of device quirks that would consume more effort than the rest of playback combined, and the platforms already provide a route to those speakers.
