//! A lyrics plugin: finds synced lyrics on LRCLIB for tracks that have none and saves
//! them beside the audio, where the scanner picks them up.

pub mod lrclib;

#[cfg(target_arch = "wasm32")]
mod plugin {
    use crate::lrclib::{self, Found, Query};

    wit_bindgen::generate!({ path: "../wit/lyrics", world: "lyrics-plugin" });

    use jewelcase::plugin::host::{self, Permission};
    use jewelcase::plugin::{http, library};

    struct Plugin;

    #[derive(Default)]
    struct Tally {
        checked: u32,
        had_lyrics: u32,
        synced: u32,
        plain: u32,
        instrumental: u32,
        missing: u32,
        failed: u32,
    }

    fn fetch(url: &str) -> Result<(u16, String), String> {
        http::get(url).map(|r| (r.status, r.body))
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
        fn run() -> Result<String, String> {
            let granted = host::granted();
            for (p, what) in [
                (Permission::LibraryRead, "read the library"),
                (Permission::Network, "use the network"),
            ] {
                if !granted.contains(&p) {
                    return Err(format!("needs permission to {what}"));
                }
            }
            // Without write access it still looks everything up, and reports what it would save.
            let can_write = granted.contains(&Permission::LibraryWrite);
            let mut n = Tally::default();
            let mut offset = 0;
            loop {
                let page = library::tracks(offset, 50)?;
                if page.is_empty() {
                    break;
                }
                offset += page.len() as u32;
                for t in &page {
                    let name = format!("{} — {}", t.title, t.artists.join(", "));
                    if t.has_lyrics {
                        n.had_lyrics += 1;
                        continue;
                    }
                    n.checked += 1;
                    let (synced, text) = match lookup(t) {
                        Ok(Found::Synced(s)) => (true, s),
                        Ok(Found::Plain(s)) => (false, s),
                        Ok(Found::Instrumental) => {
                            n.instrumental += 1;
                            host::log(&format!("♪ {name}: instrumental"));
                            continue;
                        }
                        Ok(Found::Nothing) => {
                            n.missing += 1;
                            host::log(&format!("· {name}: not found"));
                            continue;
                        }
                        Err(e) => {
                            n.failed += 1;
                            host::log(&format!("✗ {name}: {e}"));
                            continue;
                        }
                    };
                    let kind = if synced { "synced" } else { "plain" };
                    if !can_write {
                        host::log(&format!("✓ {name}: found {kind} lyrics (not saved)"));
                    } else if let Err(e) = library::save_lyrics(t.id, synced, &text) {
                        n.failed += 1;
                        host::log(&format!("✗ {name}: could not save: {e}"));
                        continue;
                    } else {
                        host::log(&format!("✓ {name}: saved {kind} lyrics"));
                    }
                    if synced { n.synced += 1 } else { n.plain += 1 }
                }
            }
            Ok(summary(&n, can_write))
        }
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
            s.push_str(" Nothing was saved: write access wasn't granted.");
        }
        s
    }

    export!(Plugin);
}
