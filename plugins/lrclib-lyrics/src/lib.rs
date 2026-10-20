//! A lyrics plugin: finds synced lyrics on LRCLIB for tracks that have none and saves
//! them beside the audio, where the scanner picks them up.

pub mod lrclib;
pub mod progress;

/// Where lyrics for the track at `path` go: beside it, with the scanner's name for them,
/// `.lrc` when synced and `.txt` when plain (`requirements/scanning.md` §3.4).
pub fn lyrics_path(path: &str, synced: bool) -> String {
    let extension = if synced { "lrc" } else { "txt" };
    let (folder, name) = path.rsplit_once('/').map_or(("", path), |(f, n)| (f, n));
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    if folder.is_empty() {
        format!("{stem}.{extension}")
    } else {
        format!("{folder}/{stem}.{extension}")
    }
}

#[cfg(target_arch = "wasm32")]
mod plugin {
    mod pass;

    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::lrclib::{self, Found, Query};
    use crate::progress::{self, Saving, Seen};
    use pass::PAGE;

    wit_bindgen::generate!({ path: "../../crates/plugins/wit", world: "plugin" });

    use jewelcase::plugin::files::{self, WriteMode};
    use jewelcase::plugin::host::{self, Error, Permission};
    use jewelcase::plugin::settings;
    use jewelcase::plugin::{http, library};

    struct Plugin;

    #[derive(Default)]
    struct Tally {
        checked: u32,
        plain_only: u32,
        had_lyrics: u32,
        synced: u32,
        plain: u32,
        instrumental: u32,
        missing: u32,
        failed: u32,
    }

    /// GETs `url`, giving the reply's status and body, or why there is no reply.
    fn fetch(url: &str) -> Result<(u16, String), String> {
        let request = http::Request {
            method: http::Method::Get,
            url: url.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        let reply = http::send(&request)?;
        Ok((reply.status, String::from_utf8_lossy(&reply.body).into_owned()))
    }

    /// Why an import did not do what it was asked, as the run's log and summary say it.
    impl From<Error> for String {
        fn from(error: Error) -> String {
            match error {
                Error::NotGranted => "a permission it needs was not granted".into(),
                Error::Refused(why) | Error::Failed(why) => why,
                Error::RateLimited(ms) => {
                    format!("the service cannot be asked again for {} s", ms.div_ceil(1000))
                }
                Error::TimedOut => "the request timed out".into(),
            }
        }
    }

    fn lookup(t: &library::Track) -> Result<Found, String> {
        let q = Query {
            title: &t.title,
            artist: t.artists.first().map(String::as_str).unwrap_or(""),
            album: t.album.as_deref(),
            duration_s: t.duration_ms as f64 / 1000.0,
        };
        let (status, body) = fetch(&lrclib::get_url(&q))?;
        let exact = lrclib::parse_get(status, &body)?;
        // The exact match may only have plain lyrics; a search often turns up synced ones.
        if exact.as_ref().is_some_and(|r| {
            r.synced_lyrics
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty())
        }) {
            return Ok(lrclib::best(exact.as_ref(), &[], q.duration_s));
        }
        let (status, body) = fetch(&lrclib::search_url(&q))?;
        let search = lrclib::parse_search(status, &body)?;
        Ok(lrclib::best(exact.as_ref(), &search, q.duration_s))
    }

    impl Guest for Plugin {
        fn handle(event: Event) -> Result<String, String> {
            // Before the permissions: settings are checked before anything is granted, and it
            // has nothing to check them against.
            if let Event::CheckSettings = event {
                return Ok("Nothing to check.".into());
            }
            let granted = host::granted();
            for (p, what) in [
                (Permission::LibraryRead, "read the library"),
                (Permission::Network, "use the network"),
            ] {
                if !granted.contains(&p) {
                    return Err(format!("needs permission to {what}"));
                }
            }
            // Without access to add files it still looks everything up, and reports what it
            // would save.
            let can_write = granted.contains(&Permission::LibraryAdd);
            let synced_only = matches!(settings::get("syncedOnly"), Some(settings::Value::Flag(true)));
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| format!("the clock is before 1970: {e}"))?
                .as_secs();
            let mut n = Tally::default();
            let saving = Saving { can_write, synced_only };
            let mut look = |t: &library::Track| visit(t, saving, &mut n);
            let walked = match event {
                // The library a part at a time, from where the last run stopped: every track
                // without lyrics, except misses not due yet.
                Event::Run | Event::Scheduled => pass::resume(now, saving, &mut look)?,
                // The tracks that were added or changed, which is every track at first.
                Event::TracksChanged(changes) => {
                    for ids in changes.changed.chunks(PAGE as usize) {
                        for t in library::get_tracks(ids)? {
                            pass::note(t.id, look(&t), now)?;
                        }
                    }
                    changes.removed.into_iter().try_for_each(pass::forget)?;
                    return Ok(summary(&n, can_write));
                }
                // It asks to hear of no other event, and settings were answered above.
                Event::ScanFinished(_)
                | Event::CheckSettings
                | Event::Played(_)
                | Event::Playing(_)
                | Event::Searched(_)
                | Event::SearchesForgotten(_)
                | Event::AlbumsChanged(_)
                | Event::ArtistsChanged(_) => {
                    return Ok("Nothing to do.".into());
                }
            };
            n.had_lyrics += walked.had_lyrics;
            let mut s = summary(&n, can_write);
            if !walked.finished {
                s.push_str(" There is more to do: it carries on from there next time.");
            }
            Ok(s)
        }
    }

    /// Looks up lyrics for `t` if it has none, saves them if it can, and says what it saw.
    fn visit(t: &library::Track, saving: Saving, n: &mut Tally) -> Seen {
        let name = format!("{} — {}", t.title, t.artists.join(", "));
        if t.has_lyrics {
            n.had_lyrics += 1;
            return Seen::Settled;
        }
        n.checked += 1;
        let found = match lookup(t) {
            Ok(found) => found,
            Err(e) => {
                n.failed += 1;
                host::log(&format!("✗ {name}: {e}"));
                return Seen::Failed;
            }
        };
        let seen = progress::seen(&found, saving);
        let (synced, text) = match found {
            Found::Synced(s) => (true, s),
            Found::Plain(_) if saving.synced_only => {
                n.plain_only += 1;
                host::log(&format!("· {name}: only plain lyrics, skipped"));
                return seen;
            }
            Found::Plain(s) => (false, s),
            Found::Instrumental => {
                n.instrumental += 1;
                host::log(&format!("♪ {name}: instrumental"));
                return seen;
            }
            Found::Nothing => {
                n.missing += 1;
                host::log(&format!("· {name}: not found"));
                return seen;
            }
        };
        let kind = if synced { "synced" } else { "plain" };
        if !saving.can_write {
            host::log(&format!("✓ {name}: found {kind} lyrics (not saved)"));
        } else if let Err(e) = save(t, synced, &text) {
            n.failed += 1;
            host::log(&format!("✗ {name}: could not save: {e}"));
            return Seen::Failed;
        } else {
            host::log(&format!("✓ {name}: saved {kind} lyrics"));
        }
        if synced { n.synced += 1 } else { n.plain += 1 }
        seen
    }

    /// Saves lyrics beside the track as a new file: lyrics already there are never replaced.
    fn save(t: &library::Track, synced: bool, text: &str) -> Result<(), String> {
        let contents = format!("{}\n", text.trim());
        let path = crate::lyrics_path(&t.path, synced);
        files::write(t.root, &path, contents.as_bytes(), WriteMode::Create).map_err(String::from)
    }

    fn summary(n: &Tally, saved: bool) -> String {
        let found = n.synced + n.plain;
        let mut s = format!(
            "{} lyrics for {found} of {} tracks ({} synced, {} plain)",
            if saved { "Saved" } else { "Found" },
            n.checked,
            n.synced,
            n.plain
        );
        let mut rest = Vec::new();
        if n.missing > 0 {
            rest.push(format!("{} not found", n.missing));
        }
        if n.plain_only > 0 {
            rest.push(format!("{} with only plain lyrics", n.plain_only));
        }
        if n.instrumental > 0 {
            rest.push(format!("{} instrumental", n.instrumental));
        }
        if n.failed > 0 {
            rest.push(format!("{} failed", n.failed));
        }
        if n.had_lyrics > 0 {
            rest.push(format!("{} already had lyrics", n.had_lyrics));
        }
        if !rest.is_empty() {
            s.push_str("; ");
            s.push_str(&rest.join(", "));
        }
        s.push('.');
        if !saved && found > 0 {
            s.push_str(" Nothing was saved: adding files wasn't granted.");
        }
        s
    }

    export!(Plugin);
}

#[cfg(test)]
mod tests {
    use super::lyrics_path;

    #[test]
    fn lyrics_go_beside_the_audio() {
        assert_eq!(
            lyrics_path("Green Day/Insomniac/06 - Brain Stew.flac", true),
            "Green Day/Insomniac/06 - Brain Stew.lrc"
        );
        assert_eq!(lyrics_path("Song.mp3", false), "Song.txt");
        assert_eq!(lyrics_path("A/B.side.flac", true), "A/B.side.lrc");
    }
}
