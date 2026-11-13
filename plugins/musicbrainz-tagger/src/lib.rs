//! A tagging plugin: finds tracks with no artist on MusicBrainz by their title and length, and
//! writes the tags they are missing into their files, where the scanner picks them up.

pub mod failure;
pub mod musicbrainz;
pub mod progress;
pub mod tags;
pub mod title;

/// The largest file it tags. Writing tags holds the file and copies of it while it rewrites
/// them, which must all fit in a plugin's 64 MiB of memory (plugins/README.md, Limits).
pub const MAX_FILE: u64 = 16 * 1024 * 1024;

/// How much of a file one read asks for: the most the host gives.
pub const READ: u32 = 8 * 1024 * 1024;

#[cfg(target_arch = "wasm32")]
mod plugin {
    mod pass;

    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    use crate::failure::Failure;
    use crate::musicbrainz::{self, Found, Match};
    use crate::progress::{RETRIES, Seen, TIME_PER_RUN};
    use crate::tags::{self, Tags};
    use crate::title;
    use crate::{MAX_FILE, READ};

    // Reads the contract from the host's copy (plugins/README.md).
    wit_bindgen::generate!({ path: "../../crates/plugins/wit", world: "plugin" });

    use jewelcase::plugin::files::{self, WriteMode};
    use jewelcase::plugin::host::{self, Error, Permission};
    use jewelcase::plugin::{http, library, settings};

    struct Plugin;

    #[derive(Default)]
    struct Tally {
        checked: u32,
        tagged: u32,
        matched: u32,
        guessed: u32,
        covers: u32,
        complete: u32,
        missing: u32,
        ambiguous: u32,
        failed: u32,
    }

    /// What this run does beyond searching, from its grants and settings.
    #[derive(Clone, Copy)]
    struct Options {
        /// Whether replacing files was granted, without which it only reports.
        can_write: bool,
        /// Whether a title naming its artist is written as it reads when nothing matches.
        guess: bool,
        /// Whether a match's cover is fetched and embedded.
        covers: bool,
    }

    impl Options {
        fn now() -> Options {
            // A setting no one saved, which has its default, counts as on.
            let on = |name| !matches!(settings::get(name), Some(settings::Value::Flag(false)));
            Options {
                can_write: host::granted().contains(&Permission::LibraryChange),
                guess: on("guessFromTitle"),
                covers: on("covers"),
            }
        }
    }

    /// GETs `url` from MusicBrainz, giving the reply's status and body, or why there is no reply.
    fn fetch(url: &str) -> Result<(u16, String), Failure> {
        let (status, body) = get(url)?;
        Ok((status, String::from_utf8_lossy(&body).into_owned()))
    }

    /// GETs `url`, naming the plugin as its client, giving the reply's status and body.
    fn get(url: &str) -> Result<(u16, Vec<u8>), Failure> {
        let request = http::Request {
            method: http::Method::Get,
            url: url.to_owned(),
            headers: vec![
                ("user-agent".into(), musicbrainz::USER_AGENT.into()),
                ("accept".into(), "application/json".into()),
            ],
            body: Vec::new(),
        };
        let reply = http::send(&request).map_err(Failure::from)?;
        Ok((reply.status, reply.body))
    }

    /// The first of `urls`, a release's covers best first, that is an image, or why none could
    /// be fetched. A cover is a nicety, so nothing about it stops the run.
    fn cover(urls: &[String]) -> Result<Option<Vec<u8>>, String> {
        for url in urls {
            match get(url) {
                Ok((200, image)) => return Ok(Some(image)),
                // The Cover Art Archive has none for it.
                Ok((404, _)) => continue,
                Ok((status, _)) => return Err(format!("the Cover Art Archive answered {status}")),
                Err(why) => return Err(why.to_string()),
            }
        }
        Ok(None)
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

    /// Searches MusicBrainz for `t`, by each of its queries in turn until one finds something.
    /// A search it turns away as too many is sent once more ([`RETRIES`]): the host pauses
    /// requests to it after one, so the retry waits for its turn.
    fn lookup(t: &library::Track) -> Result<Found, Failure> {
        let mut retries = RETRIES;
        for q in musicbrainz::queries(&t.title, t.duration_ms) {
            let url = musicbrainz::search_url(&q);
            let (mut status, mut body) = fetch(&url)?;
            while musicbrainz::turned_away(status) && retries > 0 {
                retries -= 1;
                (status, body) = fetch(&url)?;
            }
            match musicbrainz::choose(&q, &musicbrainz::parse(status, &body)?) {
                Found::Nothing => continue,
                found => return Ok(found),
            }
        }
        Ok(Found::Nothing)
    }

    impl Guest for Plugin {
        /// Runs the pass through the library on **Run now** and the schedule, and looks up the
        /// tracks a `tracks-changed` event names. It asks to hear of no other event, and has no
        /// settings to check.
        fn handle(event: Event) -> Result<String, String> {
            // Without access to replace files it still looks everything up, and reports what
            // it would write.
            let options = Options::now();
            let can_write = options.can_write;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| format!("the clock is before 1970: {e}"))?
                .as_secs();
            let mut n = Tally::default();
            let deadline = Instant::now() + TIME_PER_RUN;
            let look = |t: &library::Track| visit(t, options, deadline, &mut n);
            let more = match event {
                // The library a part at a time, from where the last run stopped: every track
                // with no artist, except misses not due yet.
                Event::Run | Event::Scheduled => {
                    let mode_since = pass::mode_since(can_write, now)?;
                    let walked = pass::resume(now, mode_since, deadline, look)?;
                    n.tagged += walked.tagged;
                    if walked.finished {
                        String::new()
                    } else {
                        " There is more to do: it carries on from there next time.".into()
                    }
                }
                // The tracks that were added or changed, which is every track at first.
                Event::TracksChanged(changes) => {
                    // Noted here too, so misses this run makes count as made under it.
                    pass::mode_since(can_write, now)?;
                    let (changed, removed) = (&changes.changed, &changes.removed);
                    match pass::changed(changed, removed, now, deadline, look)? {
                        0 => String::new(),
                        left => format!(
                            " It ran out of time with {left} to go, which the pass looks up in its turn."
                        ),
                    }
                }
                _ => return Ok("Nothing to do.".into()),
            };
            Ok(summary(&n, can_write) + &more)
        }
    }

    /// Looks `t` up if it has no artist, writes the tags it is missing if it can, and says what
    /// it saw. When MusicBrainz has nothing, a title that names its artist is written as it
    /// reads, if that is allowed. An error is a lookup that can't be answered now, which stops
    /// the run.
    fn visit(
        t: &library::Track,
        options: Options,
        deadline: Instant,
        n: &mut Tally,
    ) -> Result<Seen, String> {
        if !t.artists.is_empty() {
            n.tagged += 1;
            return Ok(Seen::Settled);
        }
        n.checked += 1;
        let name = &t.title;
        let guess = || title::read(&t.title).guess().filter(|_| options.guess);
        let (found, guessed) = match lookup(t) {
            Ok(Found::Match(found)) => (found, false),
            Ok(Found::Ambiguous(artists)) => match guess() {
                Some(tags) => (Match { tags, covers: Vec::new() }, true),
                None => {
                    n.ambiguous += 1;
                    host::log(&format!("? {name}: could be by {}", artists.join(" or ")));
                    return Ok(Seen::Missed);
                }
            },
            Ok(Found::Nothing) => match guess() {
                Some(tags) => (Match { tags, covers: Vec::new() }, true),
                None => {
                    n.missing += 1;
                    host::log(&format!("· {name}: not found"));
                    return Ok(Seen::Missed);
                }
            },
            Err(Failure::Unavailable(why)) => return Err(why),
            Err(Failure::Failed(why)) => {
                n.failed += 1;
                host::log(&format!("✗ {name}: {why}"));
                return Ok(Seen::Failed);
            }
        };
        let what = if guessed {
            format!("not on MusicBrainz, so read from the title as {}", describe(&found.tags))
        } else {
            describe(&found.tags)
        };
        let count = |n: &mut Tally| if guessed { n.guessed += 1 } else { n.matched += 1 };
        if !options.can_write {
            count(n);
            host::log(&format!("✓ {name}: {what} (not written)"));
            return Ok(Seen::Missed);
        }
        // A cover is two requests at most, each up to 75 s, so one is only started before the
        // deadline, and the run still ends inside its time limit.
        let covers =
            if options.covers && Instant::now() < deadline { &found.covers[..] } else { &[] };
        match write(t, &found.tags, covers) {
            Ok(Some(written)) => {
                count(n);
                if written.contains(&"cover") {
                    n.covers += 1;
                }
                host::log(&format!("✓ {name}: {what}; wrote {}", written.join(", ")));
                Ok(Seen::Settled)
            }
            // Its file has every tag already; its artist is only waiting to be scanned in.
            Ok(None) => {
                n.complete += 1;
                Ok(Seen::Settled)
            }
            Err(why) => {
                n.failed += 1;
                host::log(&format!("✗ {name}: {what}, but could not write it: {why}"));
                Ok(Seen::Failed)
            }
        }
    }

    /// What a match is, for the log: `Artist — Title`, and the album it is on.
    fn describe(tags: &Tags) -> String {
        let mut s = format!("{} — {}", tags.artists.join("; "), tags.title);
        if let Some(album) = &tags.album {
            s.push_str(&format!(", on {album}"));
        }
        s
    }

    /// Fills in what `t`'s file is missing from `tags`, with the first of `covers` there is if
    /// it has no picture, and replaces the file, answering the names of the tags written, or
    /// none when it had them all.
    fn write(
        t: &library::Track,
        tags: &Tags,
        covers: &[String],
    ) -> Result<Option<Vec<&'static str>>, String> {
        let file = read(t)?;
        let image = if covers.is_empty() || tags::has_cover(&file)? {
            None
        } else {
            cover(covers).unwrap_or_else(|why| {
                host::log(&format!("· {}: no cover: {why}", t.title));
                None
            })
        };
        let Some(filled) = tags::fill(file, tags, image)? else { return Ok(None) };
        files::write(t.root, &t.path, &filled.bytes, WriteMode::Replace)?;
        Ok(Some(filled.written))
    }

    /// The whole of `t`'s file, or why not, such as one over [`MAX_FILE`].
    fn read(t: &library::Track) -> Result<Vec<u8>, String> {
        let mut file = Vec::new();
        loop {
            let part = files::read(t.root, &t.path, file.len() as u64, READ)?;
            let done = part.len() < READ as usize;
            // Exactly, since doubling the capacity could take twice the file's memory.
            file.reserve_exact(part.len());
            file.extend(part);
            if file.len() as u64 > MAX_FILE {
                return Err(format!("its file is over {} MiB", MAX_FILE / 1024 / 1024));
            }
            if done {
                return Ok(file);
            }
        }
    }

    /// The run's one-paragraph result from its counts `n`; `can_write` is whether replacing
    /// files was granted, which decides whether it says tags were written or only found.
    fn summary(n: &Tally, can_write: bool) -> String {
        let mut s = format!(
            "{} tags for {} of {} tracks with no artist",
            if can_write { "Wrote" } else { "Found" },
            n.matched + n.guessed,
            n.checked
        );
        let mut rest = Vec::new();
        if n.guessed > 0 {
            rest.push(format!("{} of them read from the title, not on MusicBrainz", n.guessed));
        }
        if n.covers > 0 {
            rest.push(format!("{} with a cover", n.covers));
        }
        if n.missing > 0 {
            rest.push(format!("{} not found", n.missing));
        }
        if n.ambiguous > 0 {
            rest.push(format!("{} by more than one artist", n.ambiguous));
        }
        if n.failed > 0 {
            rest.push(format!("{} failed", n.failed));
        }
        if n.complete > 0 {
            rest.push(format!("{} already tagged, waiting to be scanned", n.complete));
        }
        if n.tagged > 0 {
            rest.push(format!("{} already had an artist", n.tagged));
        }
        if !rest.is_empty() {
            s.push_str("; ");
            s.push_str(&rest.join(", "));
        }
        s.push('.');
        if !can_write && n.matched + n.guessed > 0 {
            s.push_str(" Nothing was written: replacing files wasn't granted.");
        }
        s
    }

    export!(Plugin);
}
