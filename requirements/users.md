# User Requirements

## Overview
Jewelcase is a shared server with private listening. Several people use one installation and draw from the same libraries, but what each of them does inside it — their playlists, their queues, what they played and when, what they searched for — belongs to them alone.

The account model is deliberately small. There are no per-library roles, no permission matrices, and no group hierarchies. Access to a library is a single yes-or-no decision, and everything else follows from it.

---

## 1. Roles

Every account holds exactly one:

- **Owner** — a single account, created at first setup. Has every admin capability, plus promoting and demoting admins and transferring ownership. Cannot be demoted, suspended, or deleted by anyone, including itself. This guarantees a server can never be left with nobody able to administer it.
- **Admin** — operates the server. Manages libraries, storage, and scanning; installs and toggles plugins; manages accounts; grants and revokes library access. Cannot promote or demote other admins, or alter the owner.
- **User** — listens. Browses, searches, and plays granted libraries, and maintains their own playlists, queues, downloads, history, and statistics. No visibility into server configuration or other accounts.

Ownership may be transferred to another admin. Transfer is explicit and confirmed, and the previous owner becomes a regular admin — the role is never held by two accounts at once and never by none.

---

## 2. Account Provisioning

### 2.1 First Run
A new server has no accounts and no default credentials. First launch presents a guided setup creating the owner account. Until it completes, the server exposes nothing but the setup flow.

### 2.2 Invites
Accounts are created through **invite links**. An admin generates one, the recipient follows it and chooses their own username and password. Admins never see, set, or handle another person's credentials.

An invite supports:

- **Single-use by default**, so one link creates one user.
- **An expiry**, with a sensible default.
- **Pre-scoped library access**, so an invited person lands ready to listen with no follow-up from the admin.
- **Revocation** at any time before redemption.

Admins see all pending invites — who created each, when it expires, whether it has been used. Redeemed, revoked, and expired invites are visibly distinguishable from live ones.

### 2.3 No Self-Registration
There is no public signup. A person without an invite cannot create an account, and the server does not advertise a registration path.

---

## 3. Authentication

- **Username and password.** No additional factors and no external identity in v1.
- **No account enumeration.** Failure messaging is identical for an unknown account, a wrong password, and a suspended account.
- **Credentials are never recoverable.** Stored passwords cannot be read back by anyone, including the owner. A forgotten password is resolved by a reset, never by retrieval.
- **Password resets** are performed by the user when signed in, or initiated by an admin through a single-use expiring link. An admin never sets a password on someone's behalf.
- **Changing a password** signs out all other devices, leaving the device that made the change signed in. Those devices keep their downloads ([`offline.md` §9](offline.md#9-download-lifecycle)) — a password change is a security action, not a wipe.

### 3.1 Protection
Strong, fixed defaults chosen by the project. There is no configuration that can weaken them.

- Repeated failed sign-ins are progressively delayed, then temporarily locked out. Limits apply **per account and per origin**, so neither a targeted account nor a broad sweep is viable.
- **Lockout is always temporary and self-clearing.** No sequence of failed attempts can permanently lock a person out of their own server.
- Passwords must meet a minimum strength requirement and are rejected if they appear in known breached-password lists. No composition rules that push toward predictable patterns, and no upper length limit that would discourage passphrases.
- Sign-in attempts are recorded so admins can see an account under pressure.

---

## 4. Sessions & Devices

Every sign-in registers a **device** — a first-class named entity, not an anonymous session. This is the same identity used for remote control and playback handoff, so the list a user sees in settings and the list they cast to are the same list.

A device carries a **name** (defaulted to something recognizable, user-editable), its **type** (phone, tablet, desktop, web), **first and last seen** times, whether it is **currently connected**, and **what it is playing**.

- **Sessions are long-lived.** A music app that demands frequent re-authentication is a broken music app. A device that has been offline a long time does not lose its session for having been idle.
- **Users manage their own devices**, signing out any of them remotely, individually or all at once.
- **Admins can sign out any device on the server**, for any account.

---

## 5. Library Access

- Access is **binary** — an account has it or does not. No read-only, contributor, or curator variants.
- Admins grant and revoke per account, and in bulk.
- **Admins and the owner have access to every library** implicitly.
- A user may hold access to any number of libraries and moves between them freely.
- **Revocation is not destruction.** Playlists, history, and statistics for that library are preserved and return intact if access is restored.
- A user with access to nothing can still sign in, seeing an empty state that explains why rather than an error.

---

## 6. Profile & Preferences

User-controlled, never imposed by an admin.

- **Playback** — crossfade and its duration, volume normalization mode, silence skipping, default shuffle and repeat behavior, endless play, and whether a manual queue carries over into a new context ([`queue.md` §6](queue.md#6-sessions)). Gapless is not listed here: it is always on and not configurable ([`playback.md` §4](playback.md#4-transitions)).
- **Streaming quality** — independent choices for unmetered connections, metered connections, and offline downloads, so nobody must choose between quality at home and cost on the move. A further choice decides whether a downloaded track may be streamed when the connection allows better quality, or always plays from the device ([`offline.md` §4](offline.md#4-choosing-between-local-and-stream)).
- **Appearance** — theme, accent color, list-versus-grid density, language.
- **Identity** — display name and avatar, both editable; username, which identifies the account for sign-in.

### 6.1 Where Preferences Live
- **Account preferences follow the person.** Appearance and playback behavior apply everywhere they sign in; a change on one device appears on the others.
- **Device preferences stay put.** Streaming quality, download quality, storage budget ([`offline.md` §5](offline.md#5-storage--quality)), and **volume** ([`realtime.md` §3](realtime.md#3-shifting-control-and-waking)) are properties of the device and its connection, not the person — a phone on cellular and a desktop on ethernet must be able to disagree.

---

## 7. Privacy & Personal Data

- **Personal data is private between users.** Playlists, queues, downloads, listening history, search history, and statistics are never visible to other non-admin accounts.
- **Personal data is clearable by the user**, and clearing is permanent — nothing survives indirectly through anything derived from it ([`analytics.md` §9](analytics.md#9-privacy-and-control), [`search.md` §6](search.md#6-recent-searches)).

---

## 8. Admin Capabilities & Visibility

Admins have **full visibility into user data and no ability to act as a user.** Both halves are equally binding.

**Admins can:**
- View any account's playlists, queues, downloads, history, and statistics.
- See who is signed in, on what devices, and what is streaming now.
- Manage accounts — invite, suspend, delete, reset passwords, sign out devices, grant and revoke access.
- Export any user's data on their behalf.

**Admins cannot:**
- Sign in as, impersonate, or act on behalf of another user.
- Start, stop, or redirect playback on another user's devices.
- Create, modify, or delete another user's playlists, queues, or history.
- Read another user's password in any form.

Every user is told plainly, in the interface and not only in documentation, that admins can see their listening data. A privacy boundary people do not know the shape of is not a privacy boundary.

Administrative actions affecting an account are recorded, so the history of who changed what is available to the owner.

---

## 9. Account Lifecycle

- **Suspension.** Cannot sign in, all devices signed out, active playback stops. Everything owned is preserved untouched, and restoring returns the account exactly as it was. The owner cannot be suspended.
- **Deletion.** Explicitly confirmed, permanent, removing all personal data — playlists, queues, downloads, devices, history, statistics. It **never touches audio files, metadata, or artwork.** No account operation can damage a collection. The owner cannot be deleted; removing them requires transferring ownership first.
- **Export.** A user can export their own data in a portable, documented format useful outside Jewelcase. Admins can export on a user's behalf, including for a suspended account, so someone leaving can be given their data without their account first being restored.

---

## 10. Access Semantics

- **Content a user cannot access is indistinguishable from content that does not exist.** Probing must never reveal what a library holds, or even that it holds anything.
- **Attempting something a role does not permit fails distinctly** from the above — a user reaching an admin-only capability is told they may not, not that it is missing.
- **A suspended or deleted account is indistinguishable from one that never existed.**
- **What a request may reach is determined by the authenticated account**, never by identifiers the client supplies.
