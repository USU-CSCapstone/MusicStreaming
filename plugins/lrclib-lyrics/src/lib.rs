//! A lyrics plugin: finds synced lyrics on LRCLIB for tracks that have none and saves
//! them beside the audio, where the scanner picks them up.

pub mod lrclib;

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
    use crate::lrclib::{self, Found, Query};

    wit_bindgen::generate!({ path: "../../crates/plugins/wit", world: "plugin" });

    use jewelcase::plugin::files::{self, WriteMode};
    use jewelcase::plugin::host::{self, Permission};
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

    fn fetch(url: &str) -> Result<(u16, String), String> {
        let request = http::Request {
            method: http::Method::Get,
            url: url.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        http::send(&request).map(|r| (r.status, String::from_utf8_lossy(&r.body).into_owned()))
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
            let mut n = Tally::default();
            let mut visit = |t: &library::Track| visit(t, can_write, synced_only, &mut n);
            match event {
                // The whole library, a page at a time: asked for, or on the schedule, as
                // lrclib.net gains lyrics for tracks it had none for.
                Event::Run | Event::Scheduled => {
                    let mut after = None;
                    loop {
                        let page = library::tracks(after, 50)?;
                        let Some(last) = page.last() else { break };
                        after = Some(last.id);
                        page.iter().for_each(&mut visit);
                    }
                }
                // Only the tracks that were added or changed; removed ones need nothing.
                Event::TracksChanged(changes) => {
                    for ids in changes.changed.chunks(50) {
                        library::get_tracks(ids)?.iter().for_each(&mut visit);
                    }
                }
                // It does not ask to hear about scans.
                // It does not ask to hear about scans, and settings were answered above.
                Event::ScanFinished(_) | Event::CheckSettings => return Ok("Nothing to do.".into()),
            }
            Ok(summary(&n, can_write))
        }
    }

    /// Looks up lyrics for `t` if it has none, and saves them if it can.
    fn visit(t: &library::Track, can_write: bool, synced_only: bool, n: &mut Tally) {
        let name = format!("{} — {}", t.title, t.artists.join(", "));
        if t.has_lyrics {
            n.had_lyrics += 1;
            return;
        }
        n.checked += 1;
        let (synced, text) = match lookup(t) {
            Ok(Found::Synced(s)) => (true, s),
            Ok(Found::Plain(_)) if synced_only => {
                n.plain_only += 1;
                host::log(&format!("· {name}: only plain lyrics, skipped"));
                return;
            }
            Ok(Found::Plain(s)) => (false, s),
            Ok(Found::Instrumental) => {
                n.instrumental += 1;
                host::log(&format!("♪ {name}: instrumental"));
                return;
            }
            Ok(Found::Nothing) => {
                n.missing += 1;
                host::log(&format!("· {name}: not found"));
                return;
            }
            Err(e) => {
                n.failed += 1;
                host::log(&format!("✗ {name}: {e}"));
                return;
            }
        };
        let kind = if synced { "synced" } else { "plain" };
        if !can_write {
            host::log(&format!("✓ {name}: found {kind} lyrics (not saved)"));
        } else if let Err(e) = save(t, synced, &text) {
            n.failed += 1;
            host::log(&format!("✗ {name}: could not save: {e}"));
            return;
        } else {
            host::log(&format!("✓ {name}: saved {kind} lyrics"));
        }
        if synced { n.synced += 1 } else { n.plain += 1 }
    }

    /// Saves lyrics beside the track as a new file: lyrics already there are never replaced.
    fn save(t: &library::Track, synced: bool, text: &str) -> Result<(), String> {
        let contents = format!("{}\n", text.trim());
        let path = crate::lyrics_path(&t.path, synced);
        files::write(t.root, &path, contents.as_bytes(), WriteMode::Create)
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
