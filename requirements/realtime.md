# Realtime Requirements

## Overview
**Shift** is the ability to use any of your devices to control any of your others. Start an album on your phone, walk into the study, and shift it to the desktop mid-track. Sit down with a laptop and start music playing on the machine by the speakers without touching it.

Underneath Shift is a broader rule: **nothing in Jewelcase requires a refresh.** A page left open overnight is current in the morning. Every device shows the same truth at the same time, without polling and without the user asking.

---

## 1. Devices

- **Every signed-in client is a Shift target.** There is no pairing step, no discovery protocol, and no requirement to be on the same network — a device is reachable because it is signed in, not because it is nearby.
- **It is the same device list users already have** (`users.md` §4): the list in settings and the list you send music to are one list, with one name per device.
- **Each device shows what matters for choosing it** — its name, its type, whether it is connected, and what it is playing.
- **Disconnected devices stay visible but cannot be selected.** Hiding them would leave a user hunting for a device that is simply asleep; showing them greyed out answers the question.

---

## 2. One Player at a Time

**Exactly one device plays per account at any moment** (`playback.md` §6).

- **Starting playback anywhere takes over.** The previously playing device stops, and playback continues on the new one.
- **Takeover is legible, never mysterious.** A user whose music stops on one device can see where it went.
- **The stopped device loses nothing.** It shows what is now playing elsewhere and can take it back in one action.
- **The single queue depends on this** (`queue.md` §7). Two devices playing different things at once would make "where am I in this queue" unanswerable, so the constraint is deliberate rather than a limitation.

---

## 3. Shifting, Control, and Waking

Three operations, all available from any device, over any connection:

- **Shift.** Move playback to another device — same track, same position, same queue, continuing rather than restarting.
- **Remote control.** Drive the playing device from another: play and pause, skip, seek, shuffle, repeat, volume, and every queue edit (`queue.md` §3).
- **Waking.** A connected but idle device can be told to start playing from nothing. This is what makes Shift worth having: walk in, choose the device by the speakers, press play.

Two rules across all three:

- **The remote surface is the full surface.** Anything a user can do to playback locally, they can do to it remotely. A remote is not a reduced set of buttons.
- **Volume belongs to the device**, not the account. Turning down a phone must not turn down the desktop.

---

## 4. What Syncs Live

| | Reaches every client without a refresh |
|---|---|
| **Playback** | What is playing, position, paused or not, shuffle, repeat (`playback.md` §6) |
| **Queue** | Contents, order, and position (`queue.md` §7) |
| **Playlists** | Every edit, as it happens (`playlists.md` §5) |
| **Library** | Music added, changed, or removed by scanning (`libraries.md` §5) |
| **Personal data** | Play counts, listening history, download state (`conventions.md` §1) |
| **Admin views** | Scan progress and problems, current listeners and streams, capacity (`users.md` §8, `performance.md` §7) |

- **A small change costs a small update.** Editing one track in a 10,000-track playlist must not push the playlist to every device.
- **Open views update in place.** A user watching a scan, a playlist, or an artist page sees it change under them without losing scroll position or selection.

---

## 5. Timeliness

- **Control feels immediate.** A command issued on one device takes effect on another within **300 ms** on a healthy connection (`performance.md` §3).
- **The controlling device responds instantly**, without waiting for the target — optimistic, then reconciled (`general.md` §3.4).
- **Position never drifts.** A client watching playback for an hour still shows the right position, without the user reopening anything.
- **A failed command is visible.** If the target did not do what was asked, the controlling device says so rather than showing a state that never happened.

---

## 6. Connection Lifecycle

- **Reconnection is automatic and silent.** Clients recover from dropped connections without user action and without an interruption to report.
- **Catching up means current state, not replayed history.** A client that has been away sees where things are now, not a backlog of what it missed.
- **Stale is never shown as live.** A client that has lost its connection says so, rather than displaying old state as though it were current.
- **Losing the playing device stops playback and preserves the state.** If the machine that was playing goes away, other devices see playback end, and any of them can pick it up from that position.
- **Sessions survive long absences** (`users.md` §4). A device closed for a month rejoins without signing in again.

---

## 7. Conflicting Commands

- **Everything converges.** Two devices acting at once may briefly disagree, but they always settle on the same state — never two devices confidently showing different truths.
- **The last command wins** for playback control. Two people pausing at once produces a paused player, not an argument.
- **Playlist edits are the exception**, and follow their own rule: an edit applies only against the version the user was looking at (`playlists.md` §5).
- **A correction the user can see is explained.** Where reconciliation undoes something visible, the user is told rather than watching it silently revert.

---

## 8. Boundaries

- **Realtime is scoped to the account.** A client receives events for its own account and its own libraries, never anything else (`general.md` §3.6).
- **Admins observe, never control** (`users.md` §8). Admin views update live with who is listening and what is streaming, but no admin can transfer, start, stop, or redirect another user's playback.
- **Offline devices are not a special case.** A device with no connection behaves per `offline.md`, and rejoins by reconciling like any other client (`offline.md` §8).
