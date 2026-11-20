//! The lyrics section of a search (`design/search.md` §2): the tracks whose lyrics have a line
//! holding every word of the query, by their best lines, ranked in the core's two steps, each
//! shown with its line and the lines around it.
//!
//! Step one matches the library's lyric index ([`LyricIndex::search`]) and is the costly one: a
//! common word holds a large share of every line. So a query too short to say much searches no
//! lyrics, and the last few searches' step-one matches are kept, everything they found, to
//! narrow to what each caller's search keeps: a page after the first, a tab, or the same words
//! again match no line again. Step two reads back the stored lyrics of the best
//! [`PHRASE_TRACKS`] kept and lifts those holding the query as a phrase ([`lift_phrases`]).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};

use jewelcase_core::{
    LyricHit, LyricIndex, PHRASE_TRACKS, Recover, is_lyric_line, lift_phrases, plain_lines,
    wants_phrases, words,
};
use jewelcase_scanner::SyncedLine;
use rusqlite::Connection;

use jewelcase_catalog::by_ids::by_ids;
use jewelcase_catalog::search::LyricMatch;

use super::Indexes;
use crate::api::{Code, Problem};
use crate::db::{self, Database};
use jewelcase_catalog::tracks::{self, TrackSummary};

/// A query whose words hold fewer characters searches no lyrics: its first letters are in about
/// half of a large library's lines, and match none that a word more would not.
pub const MIN_QUERY_CHARS: usize = 3;

/// How many searches' step-one matches are kept: a few, since one that matches every track of
/// the heavy case holds about 6 MB.
const KEPT: usize = 4;

/// Whether `q` searches lyrics: [`MIN_QUERY_CHARS`] or more in its words.
pub fn searches(q: &str) -> bool {
    words(q).iter().map(|word| word.chars().count()).sum::<usize>() >= MIN_QUERY_CHARS
}

/// The step-one matches of the last few searches of any library, before anything is left out.
#[derive(Default)]
pub struct Matches {
    recent: Mutex<VecDeque<Searched>>,
}

/// One search's step-one matches.
struct Searched {
    library: i64,
    /// The index they were matched in: another, rebuilt since, matches anew.
    index: Weak<LyricIndex>,
    /// The query's words, which are what is matched.
    words: Vec<String>,
    hits: Arc<Vec<LyricHit>>,
}

impl Matches {
    /// Every track `index`, `library`'s, finds for `q`, by step one, matched again only when the
    /// last few searches did not match the same words in it.
    async fn get(
        &self,
        library: i64,
        index: Arc<LyricIndex>,
        q: &str,
    ) -> Result<Arc<Vec<LyricHit>>, Problem> {
        let words = words(q);
        let same = |searched: &Searched| {
            searched.library == library
                && Weak::ptr_eq(&searched.index, &Arc::downgrade(&index))
                && searched.words == words
        };
        if let Some(searched) = self.recent.lock().recover().iter().find(|s| same(s)) {
            return Ok(searched.hits.clone());
        }
        let (q, weak) = (q.to_owned(), Arc::downgrade(&index));
        // Tens of milliseconds for a common word at full scale.
        let hits = tokio::task::spawn_blocking(move || index.search(&q, |_| true))
            .await
            .map_err(|_| Problem::new(Code::Internal))?;
        let hits = Arc::new(hits);
        let mut recent = self.recent.lock().recover();
        recent.retain(|searched| searched.index.strong_count() > 0);
        recent.push_front(Searched { library, index: weak, words, hits: hits.clone() });
        recent.truncate(KEPT);
        Ok(hits)
    }
}

/// A search's lyric matches, in the section's order: those step two lifted first, then the rest.
pub struct Ranked {
    pub hits: Vec<LyricHit>,
    /// How many of `hits` lead because step two lifted them.
    pub lifted: usize,
}

/// Where a match is in a section's order, which a page continues after: lifted or not, then the
/// match, as [`Ranked`] orders them.
pub type Place = (bool, LyricHit);

impl Ranked {
    /// The place of `hits[i]`.
    pub fn place(&self, i: usize) -> Place {
        (i >= self.lifted, self.hits[i].clone())
    }

    /// How many matches come at or before `place`, so the page after it starts there.
    pub fn after(&self, place: &Place) -> usize {
        let (rest, hit) = place;
        let lifted = &self.hits[..self.lifted];
        if *rest {
            self.lifted + self.hits[self.lifted..].partition_point(|other| other <= hit)
        } else {
            lifted.partition_point(|other| other <= hit)
        }
    }
}

/// The lyric matches of `q` in `library` of the tracks `keep` keeps, ranked in both steps. A query
/// [`searches`] does not has none.
pub async fn ranked(
    db: &Arc<Database>,
    indexes: &Indexes,
    library: i64,
    q: &str,
    keep: impl Fn(i64) -> bool,
) -> Result<Ranked, Problem> {
    if !searches(q) {
        return Ok(Ranked { hits: Vec::new(), lifted: 0 });
    }
    let index = indexes.lyrics(db, library).await?;
    let all = indexes.lyric_matches.get(library, index, q).await?;
    let mut hits: Vec<LyricHit> = all.iter().filter(|hit| keep(hit.track)).cloned().collect();
    if !wants_phrases(q) {
        return Ok(Ranked { hits, lifted: 0 });
    }
    let read: Vec<i64> = hits.iter().take(PHRASE_TRACKS).map(|hit| hit.track).collect();
    let stored = db.read(move |conn| stored(conn, library, &read)).await?;
    let q = q.to_owned();
    // A few milliseconds folding the lines read back.
    tokio::task::spawn_blocking(move || {
        let lifted = lift_phrases(&q, &mut hits, |track| {
            stored.get(&track).map(|lyrics| lyrics.lines.iter().map(String::as_str).collect())
        });
        Ranked { hits, lifted }
    })
    .await
    .map_err(|_| Problem::new(Code::Internal))
}

/// A track's lyrics as stored, as lines: each timed line with when it starts, or plain lyrics
/// split at their line breaks. The lyric index numbers lines so, and so do the matches shown.
pub struct Stored {
    pub lines: Vec<String>,
    /// When each line starts, for timed lyrics.
    pub starts: Option<Vec<u64>>,
}

impl Stored {
    /// The lyrics stored as `plain` text or `synced` lines.
    pub fn new(plain: Option<String>, synced: Option<Vec<SyncedLine>>) -> Stored {
        match (synced, plain) {
            (Some(synced), _) => {
                let starts = synced.iter().map(|line| line.start_ms).collect();
                let lines = synced.into_iter().map(|line| line.text).collect();
                Stored { lines, starts: Some(starts) }
            }
            (None, plain) => Stored {
                lines: plain
                    .as_deref()
                    .map_or_else(Vec::new, |plain| plain_lines(plain).map(str::to_owned).collect()),
                starts: None,
            },
        }
    }

    /// The lyric line nearest line `at` in the direction `step` goes, if any: one of context.
    fn beside(&self, at: usize, step: impl Fn(usize) -> Option<usize>) -> Vec<String> {
        let mut at = at;
        while let Some(next) = step(at).filter(|&next| next < self.lines.len()) {
            if is_lyric_line(&self.lines[next]) {
                return vec![self.lines[next].clone()];
            }
            at = next;
        }
        Vec::new()
    }
}

/// The stored lyrics, by track, of those of `tracks` in `library` that have any.
pub fn stored(
    conn: &Connection,
    library: i64,
    tracks: &[i64],
) -> rusqlite::Result<HashMap<i64, Stored>> {
    conn.prepare_cached(
        "SELECT l.track_id, l.plain, l.synced FROM json_each(?2) AS ids \
         CROSS JOIN track_lyrics l ON l.track_id = ids.value AND l.library_id = ?1",
    )?
    .query_map(rusqlite::params![library, db::Json(tracks)], |row| {
        let synced: Option<db::Json<Vec<SyncedLine>>> = row.get(2)?;
        Ok((row.get(0)?, Stored::new(row.get(1)?, synced.map(|db::Json(lines)| lines))))
    })?
    .collect()
}

/// The matches `hits` as `user` sees their tracks, each with its line and one line of lyrics on
/// either side. A track or line gone since the index was built is left out.
pub fn matches(
    conn: &Connection,
    user: i64,
    library: i64,
    hits: &[LyricHit],
) -> rusqlite::Result<Vec<LyricMatch>> {
    let ids: Vec<i64> = hits.iter().map(|hit| hit.track).collect();
    let mut summaries =
        by_ids(conn, tracks::SELECT, "tracks t", tracks::JOINS, library, &ids, |row| {
            Ok((row.get::<_, i64>(0)?, tracks::list_summary(row)?))
        })?;
    tracks::mark_liked(conn, user, library, summaries.iter_mut().map(|(_, track)| track))?;
    let mut summaries: HashMap<i64, TrackSummary> = summaries.into_iter().collect();
    let stored = stored(conn, library, &ids)?;
    let mut found = Vec::with_capacity(hits.len());
    for hit in hits {
        let at = usize::from(hit.line);
        let (Some(lyrics), Some(track)) = (stored.get(&hit.track), summaries.remove(&hit.track))
        else {
            continue;
        };
        let Some(text) = lyrics.lines.get(at) else { continue };
        let start_ms = lyrics.starts.as_ref().and_then(|starts| starts.get(at).copied());
        let before = lyrics.beside(at, |at| at.checked_sub(1));
        let after = lyrics.beside(at, |at| at.checked_add(1));
        found.push(LyricMatch::new(track, text.clone(), start_ms, before, after));
    }
    Ok(found)
}

#[cfg(test)]
mod tests;
