//! A lyrics plugin: finds synced lyrics on LRCLIB for tracks that have none and saves
//! them beside the audio, where the scanner picks them up.

pub mod failure;
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

    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    use crate::failure::Failure;
    use crate::lrclib::{self, Found, Query};
    use crate::progress::{self, Saving, Seen, TIME_PER_RUN};

    // Reads the contract from the host's copy (plugins/README.md).
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
    fn fetch(url: &str) -> Result<(u16, String), Failure> {
        let request = http::Request {
            method: http::Method::Get,
            url: url.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        let reply = http::send(&request).map_err(Failure::from)?;
        Ok((reply.status, String::from_utf8_lossy(&reply.body).into_owned()))
    }

    /// What a failed import comes to: everything but a refusal may work later, so it stops the
    /// run.
    impl From<Error> for Failure {
        fn from(error: Error) -> Failure {
            match error {
                Error::Refused(why) => Failure::Failed(why),
                other => Failure::Unavailable(String::from(other)),
            }
        }
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

    /// Asks LRCLIB for `t`'s lyrics: the exact match, then a search unless that had synced ones.
    fn lookup(t: &library::Track) -> Result<Found, Failure> {
        let q = Query {
            title: &t.title,
            artist: t.artists.first().map(String::as_str).unwrap_or(""),
            album: t.album.as_deref(),
            duration_s: t.duration_ms as f64 / 1000.0,
        };
        let (status, body) = fetch(&lrclib::get_url(&q))?;
        let exact = lrclib::parse_get(status, &body)?;
        // The exact match may only have plain lyrics; a search often turns up synced ones.
        if exact
            .as_ref()
            .is_some_and(|r| r.synced_lyrics.as_deref().is_some_and(|s| !s.trim().is_empty()))
        {
            return Ok(lrclib::best(exact.as_ref(), &[], q.duration_s));
        }
        let (status, body) = fetch(&lrclib::search_url(&q))?;
        let search = lrclib::parse_search(status, &body)?;
        Ok(lrclib::best(exact.as_ref(), &search, q.duration_s))
    }

    impl Guest for Plugin {
        /// Runs the pass through the library on **Run now** and the schedule, and looks up the
        /// tracks a `tracks-changed` event names. It asks to hear of no other event, and has no
        /// settings to check.
        fn handle(event: Event) -> Result<String, String> {
            // Without access to add files it still looks everything up, and reports what it
            // would save.
            let can_write = host::granted().contains(&Permission::LibraryAdd);
            let synced_only =
                matches!(settings::get("syncedOnly"), Some(settings::Value::Flag(true)));
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| format!("the clock is before 1970: {e}"))?
                .as_secs();
            let mut n = Tally::default();
            let saving = Saving { can_write, synced_only };
            let look = |t: &library::Track| visit(t, saving, &mut n);
            let deadline = Instant::now() + TIME_PER_RUN;
            let more = match event {
                // What a change had no time for, then the library a part at a time, from where
                // the last run stopped: every track without lyrics, except misses not due yet.
                Event::Run | Event::Scheduled => {
                    let walked = pass::resume(now, deadline, look)?;
                    n.had_lyrics += walked.had_lyrics;
                    if walked.finished {
                        String::new()
                    } else {
                        " There is more to do: it carries on from there next time.".into()
                    }
                }
                // The tracks that were added or changed, which is every track at first.
                Event::TracksChanged(changes) => {
                    let (changed, removed) = (&changes.changed, &changes.removed);
                    match pass::changed(changed, removed, now, deadline, look)? {
                        0 => String::new(),
                        left => format!(
                            " It ran out of time with {left} to go, which the next run looks up first."
                        ),
                    }
                }
                _ => return Ok("Nothing to do.".into()),
            };
            Ok(summary(&n, can_write) + &more)
        }
    }

    /// Looks up lyrics for `t` if it has none, saves them if it can, and says what it saw. An
    /// error is a lookup that can't be answered now, which stops the run.
    fn visit(t: &library::Track, saving: Saving, n: &mut Tally) -> Result<Seen, String> {
        let name = format!("{} — {}", t.title, t.artists.join(", "));
        if t.has_lyrics {
            n.had_lyrics += 1;
            return Ok(Seen::Settled);
        }
        n.checked += 1;
        let found = match lookup(t) {
            Ok(found) => found,
            Err(Failure::Unavailable(why)) => return Err(why),
            Err(Failure::Failed(why)) => {
                n.failed += 1;
                host::log(&format!("✗ {name}: {why}"));
                return Ok(Seen::Failed);
            }
        };
        let seen = progress::seen(&found, saving);
        let (synced, text) = match found {
            Found::Synced(s) => (true, s),
            Found::Plain(_) if saving.synced_only => {
                n.plain_only += 1;
                host::log(&format!("· {name}: only plain lyrics, skipped"));
                return Ok(seen);
            }
            Found::Plain(s) => (false, s),
            Found::Instrumental => {
                n.instrumental += 1;
                host::log(&format!("♪ {name}: instrumental"));
                return Ok(seen);
            }
            Found::Nothing => {
                n.missing += 1;
                host::log(&format!("· {name}: not found"));
                return Ok(seen);
            }
        };
        let kind = if synced { "synced" } else { "plain" };
        if !saving.can_write {
            host::log(&format!("✓ {name}: found {kind} lyrics (not saved)"));
        } else if let Err(e) = save(t, synced, &text) {
            n.failed += 1;
            host::log(&format!("✗ {name}: could not save: {e}"));
            return Ok(Seen::Failed);
        } else {
            host::log(&format!("✓ {name}: saved {kind} lyrics"));
        }
        if synced {
            n.synced += 1
        } else {
            n.plain += 1
        }
        Ok(seen)
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
