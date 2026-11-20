//! Finding a song by a line of its lyrics (`design/search.md` §2.2–§2.3): the lines that hold
//! every word of the query, a track's best line for it, and the tracks in order.
//!
//! Every word of the query must match a word of one line, in any order: exactly, with the typos a
//! word as long may have and its first letter right, as a name's words match ([`matching_words`]),
//! or, the query's last word only, as a prefix, as it is being typed. A leading article in a query
//! of more than one word may go unmatched, as for names.
//!
//! A line ranks by fewer typos, then the query's leading article matched, then fewer words matched
//! only as a prefix, then fewer words in the line beyond those matched; a track by its best line,
//! the earliest of equals; and tracks by their best lines, then by ID. That is the first of the
//! two steps that rank lyrics: the index knows which words a line has but not their order, so the
//! tracks with a line that holds the query as a phrase are lifted afterwards, from their text
//! ([`lift_phrases`]).

use super::Section;
use super::lyrics::{LyricIndex, is_label};
use super::query::{
    MAX_QUERY_WORDS, Quality, Score, leading_article, matching_words, word_quality,
};
use super::words;
use crate::fold::ARTICLES;

/// How well a line matched the query: lower is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct LyricScore {
    pub(super) typos: u32,
    /// The query's leading article went unmatched.
    pub(super) skipped: bool,
    pub(super) prefixes: u32,
    /// Words in the line beyond those the query matched.
    pub(super) extra_words: u32,
}

impl LyricHit {
    /// The hit as bytes, for a client to hand back as a cursor.
    pub fn to_bytes(&self) -> Vec<u8> {
        let LyricScore { typos, skipped, prefixes, extra_words } = self.score;
        let mut bytes = vec![u8::from(skipped)];
        for n in [typos, prefixes, extra_words] {
            bytes.extend(n.to_be_bytes());
        }
        bytes.extend(self.track.to_be_bytes());
        bytes.extend(self.line.to_be_bytes());
        bytes
    }

    /// The hit [`LyricHit::to_bytes`] wrote, or `None` for anything else. A hit made up rather
    /// than read back only chooses where a page starts.
    pub fn from_bytes(bytes: &[u8]) -> Option<LyricHit> {
        let (&[skipped], rest) = bytes.split_first_chunk::<1>()?;
        let skipped = match skipped {
            0 => false,
            1 => true,
            _ => return None,
        };
        let (typos, rest) = rest.split_first_chunk::<4>()?;
        let (prefixes, rest) = rest.split_first_chunk::<4>()?;
        let (extra_words, rest) = rest.split_first_chunk::<4>()?;
        let (track, rest) = rest.split_first_chunk::<8>()?;
        let line: [u8; 2] = rest.try_into().ok()?;
        Some(LyricHit {
            score: LyricScore {
                typos: u32::from_be_bytes(*typos),
                skipped,
                prefixes: u32::from_be_bytes(*prefixes),
                extra_words: u32::from_be_bytes(*extra_words),
            },
            track: i64::from_be_bytes(*track),
            line: u16::from_be_bytes(line),
        })
    }
}

impl LyricScore {
    /// The score of a line of `length` words, up to 255, that the query's words matched by
    /// `qualities`, its leading article among them unless `skipped`.
    fn of(qualities: &[Quality], skipped: bool, length: u8) -> LyricScore {
        let (typos, prefixes) = qualities.iter().fold((0, 0), |(typos, prefixes), &q| {
            (typos + u32::from((q - 1) / 2), prefixes + u32::from((q - 1) % 2))
        });
        LyricScore {
            typos,
            skipped,
            prefixes,
            extra_words: u32::from(length).saturating_sub(qualities.len() as u32),
        }
    }
}

/// A track a search of its lyrics found, by its best line. Hits order as the section does.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LyricHit {
    pub(super) score: LyricScore,
    /// The track, by its ID.
    pub track: i64,
    /// The line's place among the track's lines, as they were given to the index.
    pub line: u16,
}

impl LyricIndex {
    /// Every track whose lyrics have a line that holds every word of `query`, and that `keep`
    /// keeps, by its best line, best first. A leading article in a query of more than one word
    /// need not match.
    pub fn search(&self, query: &str, keep: impl Fn(i64) -> bool) -> Vec<LyricHit> {
        let (query_words, optional) = query_words(query);
        let Some(last) = query_words.len().checked_sub(1) else {
            return Vec::new();
        };

        // For each query word, the lines it matches, ascending, each with how well.
        let matched: Vec<Vec<(u32, Quality)>> = query_words
            .iter()
            .enumerate()
            .map(|(i, word)| self.matching_lines(word, i == last))
            .collect();
        let (articles, required) = matched.split_at(optional);
        // The lines every required word matches: those of the fewest, checked against the rest.
        let Some(fewest) = (0..required.len()).min_by_key(|&i| required[i].len()) else {
            return Vec::new();
        };

        // The candidates are ascending, as is every list, so each list is walked once from a
        // cursor of its own rather than searched from its start for every candidate.
        let mut cursors = vec![0; matched.len()];
        let (article_cursors, required_cursors) = cursors.split_at_mut(optional);
        let mut tracks = self.tracks_from(0);
        let mut hits: Vec<LyricHit> = Vec::new();
        let mut qualities: Vec<Quality> = Vec::with_capacity(matched.len());
        'lines: for &(line, quality) in &required[fewest] {
            qualities.clear();
            qualities.push(quality);
            for (i, (lines, cursor)) in required.iter().zip(required_cursors.iter_mut()).enumerate()
            {
                if i == fewest {
                    continue;
                }
                match quality_from(lines, cursor, line) {
                    Some(q) => qualities.push(q),
                    None => continue 'lines,
                }
            }
            let article = articles
                .iter()
                .zip(article_cursors.iter_mut())
                .find_map(|(lines, cursor)| quality_from(lines, cursor, line));
            qualities.extend(article);
            let (track, at) = tracks.place(line);
            if !keep(track) {
                continue;
            }
            let skipped = optional > 0 && article.is_none();
            let score = LyricScore::of(&qualities, skipped, self.length(line));
            let hit = LyricHit { score, track, line: at };
            // Lines come in track order, so a track's lines come together.
            match hits.last_mut() {
                Some(best) if best.track == track => {
                    if hit < *best {
                        *best = hit;
                    }
                }
                _ => hits.push(hit),
            }
        }
        hits.sort_unstable();
        hits
    }

    /// The lines `word` matches, ascending, each with how well: by a word it matches exactly or
    /// with typos, or as a prefix if it is the word being typed, `typing`.
    fn matching_lines(&self, word: &str, typing: bool) -> Vec<(u32, Quality)> {
        let words: Vec<(u32, Quality)> = matching_words(self.vocabulary(), word)
            .into_iter()
            .filter(|&(_, quality)| typing || (quality - 1) % 2 == 0)
            .collect();
        let mut lines: Vec<(u32, Quality)> = Vec::new();
        for &(w, quality) in &words {
            lines.extend(self.lines_of(w).map(|line| (line, quality)));
        }
        // One word's lines are ascending already, each once; several words' are merged, each line
        // once, by the best of the words it matched by: sorted if they are few, and if they are
        // many, a common word's, through a slot for every line, which is quicker than sorting them.
        if words.len() > 1 {
            if lines.len() < self.lines() / 16 {
                lines.sort_unstable();
                lines.dedup_by_key(|&mut (line, _)| line);
            } else {
                let mut best = vec![Quality::MAX; self.lines()];
                for &(line, quality) in &lines {
                    let slot = &mut best[line as usize];
                    *slot = (*slot).min(quality);
                }
                lines.clear();
                lines.extend((0..).zip(best).filter(|&(_, quality)| quality != Quality::MAX));
            }
        }
        lines
    }
}

/// How well `line` is matched among `lines`, ascending, if it is, looking from `cursor` on and
/// leaving it there: lines asked for in ascending order walk the list once.
fn quality_from(lines: &[(u32, Quality)], cursor: &mut usize, line: u32) -> Option<Quality> {
    let rest = &lines[*cursor..];
    // Galloping: the next line is usually near, and a far one costs the logarithm of how far.
    let mut step = 1;
    while step < rest.len() && rest[step].0 < line {
        step *= 2;
    }
    *cursor += rest[..(step + 1).min(rest.len())].partition_point(|&(l, _)| l < line);
    lines.get(*cursor).filter(|&&(l, _)| l == line).map(|&(_, quality)| quality)
}

/// Where a section of lyrics whose first match is `first` goes among the sections of names
/// `sections`, in the order a search gave them (`design/search.md` §2.3): as though its best were
/// never a name matched exactly, so that a section whose best is exact comes first, and against
/// names matched otherwise as they compare among themselves; after them, when as good.
pub fn lyrics_place(sections: &[Section], first: &LyricHit) -> usize {
    let LyricScore { typos, skipped, prefixes, extra_words } = first.score;
    let lyric = Score { inexact: true, typos, skipped, prefixes, extra_words };
    sections.partition_point(|section| section.best <= lyric)
}

/// How many of the best tracks step two reads back to lift those that hold the query as a phrase.
pub const PHRASE_TRACKS: usize = 200;

/// Step two of ranking lyrics (`design/search.md` §2.3): of the first [`PHRASE_TRACKS`] of `hits`,
/// in the order [`LyricIndex::search`] gave them for `query`, the tracks with a line that holds
/// the query's words in order and together come first, each by its best such line, which may not
/// be the line step one chose, ranked as step one ranks lines; the rest keep their order after
/// them. A leading article that may go unmatched is not part of the phrase, though matching it
/// anywhere in the line still counts.
///
/// `lyrics` gives a track's lines as they were given to the index, so places agree. It is asked
/// only when the query [`wants_phrases`], once for each track considered; a track it gives none
/// for is not lifted. Returns how many were lifted, now the first of `hits`.
pub fn lift_phrases<S: AsRef<str>>(
    query: &str,
    hits: &mut [LyricHit],
    mut lyrics: impl FnMut(i64) -> Option<Vec<S>>,
) -> usize {
    let (query_words, optional) = query_words(query);
    if query_words.len() - optional < 2 {
        return 0;
    }
    let last = query_words.len() - 1;
    // Each query word, its characters, and whether it is the one being typed.
    let phrase: Vec<(&str, Vec<char>, bool)> = query_words
        .iter()
        .enumerate()
        .map(|(i, word)| (word.as_str(), word.chars().collect(), i == last))
        .collect();

    let considered = hits.len().min(PHRASE_TRACKS);
    let mut lifted: Vec<LyricHit> = Vec::new();
    let mut rest: Vec<LyricHit> = Vec::new();
    for hit in &hits[..considered] {
        let found = lyrics(hit.track).and_then(|lines| {
            let lines = lines.iter().take(usize::from(u16::MAX) + 1).map(AsRef::as_ref);
            (0..)
                .zip(lines)
                .filter(|(_, line)| !is_label(line))
                .filter_map(|(at, line)| {
                    let score = phrase_score(&phrase, optional, &words(line))?;
                    Some(LyricHit { score, track: hit.track, line: at })
                })
                .min()
        });
        match found {
            Some(lifted_hit) => lifted.push(lifted_hit),
            None => rest.push(hit.clone()),
        }
    }
    lifted.sort_unstable();
    let count = lifted.len();
    for (slot, hit) in hits.iter_mut().zip(lifted.into_iter().chain(rest)) {
        *slot = hit;
    }
    count
}

/// Whether [`lift_phrases`] reads lyrics back for `query`: when it has two words or more to put
/// in order, a leading article that may go unmatched aside. A caller that reads the lyrics first
/// reads them only then.
pub fn wants_phrases(query: &str) -> bool {
    let (query_words, optional) = query_words(query);
    query_words.len() - optional >= 2
}

/// The words of `query` that are searched, and how many lead as an article that may go unmatched.
fn query_words(query: &str) -> (Vec<String>, usize) {
    let mut query_words = words(query);
    query_words.truncate(MAX_QUERY_WORDS);
    let optional = leading_article(&query_words, |word| ARTICLES.contains(&word.as_str()));
    (query_words, optional)
}

/// The score of a line of `line` words if they hold the words of `phrase` after its first
/// `optional`, an article, in order and together: the best such run, with the article matched if
/// any of the line's words is it.
fn phrase_score(
    phrase: &[(&str, Vec<char>, bool)],
    optional: usize,
    line: &[String],
) -> Option<LyricScore> {
    let (article, required) = phrase.split_at(optional);
    let matches = |(query, chars, typing): &(&str, Vec<char>, bool), word: &str| {
        word_quality(query, chars, word).filter(|&quality| *typing || (quality - 1) % 2 == 0)
    };
    let mut qualities: Vec<Quality> = line
        .windows(required.len())
        .filter_map(|run| {
            required.iter().zip(run).map(|(query, word)| matches(query, word)).collect()
        })
        .min_by_key(|qualities: &Vec<Quality>| LyricScore::of(qualities, false, 0))?;
    let article =
        article.first().and_then(|query| line.iter().filter_map(|word| matches(query, word)).min());
    qualities.extend(article);
    let length = u8::try_from(line.len()).unwrap_or(u8::MAX);
    Some(LyricScore::of(&qualities, optional > 0 && article.is_none(), length))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lyrics of a few tracks whose lines share words.
    fn songs() -> LyricIndex {
        LyricIndex::build([
            (1, vec!["Say it ain't so", "I will not go", "Turn the lights off"]),
            (2, vec!["Hello from the other side", "I must have called a thousand times"]),
            (3, vec!["So say we all", "It ain't over", "say it like you mean it"]),
            (
                4,
                vec![
                    "Blue jeans, white shirt",
                    "Walked into the room you know you made my eyes burn",
                ],
            ),
            (5, vec!["The thousand times I said", "I called you a thousand times"]),
        ])
    }

    /// The tracks found for `query`, with their best lines' places.
    fn find(index: &LyricIndex, query: &str) -> Vec<(i64, u16)> {
        index.search(query, |_| true).iter().map(|hit| (hit.track, hit.line)).collect()
    }

    #[test]
    fn a_line_holding_every_word_finds_its_track() {
        let index = songs();
        assert_eq!(find(&index, "lights off"), [(1, 2)]);
        // In any order, and folded as names are.
        assert_eq!(find(&index, "off lights"), [(1, 2)]);
        // The shorter line first.
        assert_eq!(find(&index, "AIN'T"), [(3, 1), (1, 0)]);
        // Every word in one line: say and over are only in different lines of track 3.
        assert_eq!(find(&index, "say over"), []);
    }

    #[test]
    fn a_half_remembered_line_is_forgiven_its_typos() {
        let index = songs();
        assert_eq!(find(&index, "thousnad times"), [(5, 0), (2, 1)]);
        assert_eq!(find(&index, "helo from the other side"), [(2, 0)]);
        // The first letter must be right, as in a name.
        assert_eq!(find(&index, "jello from the other side"), []);
    }

    #[test]
    fn only_the_word_being_typed_matches_as_a_prefix() {
        let index = songs();
        assert_eq!(find(&index, "other si"), [(2, 0)]);
        assert_eq!(find(&index, "oth side"), [], "an earlier word is whole");
    }

    #[test]
    fn a_track_is_found_once_by_its_best_line() {
        let index = songs();
        // Both of track 5's lines have `thousand`, but only the second `called`; it is shorter than
        // track 2's.
        assert_eq!(find(&index, "called thousand"), [(5, 1), (2, 1)]);
        // Both of track 5's lines hold these; the first, shorter, is its best.
        assert_eq!(find(&index, "thousand times"), [(5, 0), (2, 1)]);
    }

    #[test]
    fn lines_rank_by_typos_then_prefixes_then_their_length() {
        let index = LyricIndex::build([
            (1, vec!["love me tender love me sweet"]),
            (2, vec!["love me do"]),
            (3, vec!["lovely me"]),
            (4, vec!["lobe me"]),
        ]);
        // Exact and short first; then exact and long; then a word matched as a prefix; and the
        // typo last.
        assert_eq!(find(&index, "love me"), [(2, 0), (1, 0), (4, 0)]);
        assert_eq!(find(&index, "me lov"), [(3, 0), (2, 0), (1, 0)], "a prefix only last");
        assert_eq!(find(&index, "me love"), [(2, 0), (1, 0), (3, 0), (4, 0)]);
    }

    #[test]
    fn a_leading_article_may_go_unmatched_but_ranks_lower() {
        let index =
            LyricIndex::build([(1, vec!["other side of the world"]), (2, vec!["the other side"])]);
        assert_eq!(find(&index, "the other side"), [(2, 0), (1, 0)]);
        let index = LyricIndex::build([(1, vec!["other side"])]);
        assert_eq!(find(&index, "the other side"), [(1, 0)]);
    }

    #[test]
    fn tracks_added_in_any_order_tie_by_their_ids() {
        let index = LyricIndex::build([(9, vec!["so long"]), (2, vec!["so long"])]);
        assert_eq!(find(&index, "so long"), [(2, 0), (9, 0)]);
    }

    #[test]
    fn what_keep_leaves_out_is_not_found() {
        let index = songs();
        assert_eq!(index.search("ain't", |track| track != 1).len(), 1);
        assert!(index.search("", |_| true).is_empty());
        assert!(index.search("?!", |_| true).is_empty());
        assert!(LyricIndex::default().search("so", |_| true).is_empty());
    }

    /// The tracks found for `query`, with their lines' places, after step two, `tracks` being
    /// what the index was built of.
    fn lifted(tracks: &[(i64, Vec<&str>)], query: &str) -> Vec<(i64, u16)> {
        let index = LyricIndex::build(tracks.iter().cloned());
        let mut hits = index.search(query, |_| true);
        lift_phrases(query, &mut hits, |track| {
            tracks.iter().find(|(id, _)| *id == track).map(|(_, lines)| lines.clone())
        });
        hits.iter().map(|hit| (hit.track, hit.line)).collect()
    }

    #[test]
    fn a_line_holding_the_query_in_order_and_together_comes_first() {
        let tracks = [
            (1, vec!["side the other"]),
            (2, vec!["hello from the other side"]),
            (3, vec!["other people side"]),
        ];
        // Step one ranks the shortest lines first; step two lifts the only phrase.
        assert_eq!(
            find(&LyricIndex::build(tracks.clone()), "other side"),
            [(1, 0), (3, 0), (2, 0)]
        );
        assert_eq!(lifted(&tracks, "other side"), [(2, 0), (1, 0), (3, 0)]);
    }

    #[test]
    fn lifted_tracks_rank_as_lines_do_and_the_rest_keep_their_order() {
        let tracks = [
            (1, vec!["side other"]),
            (2, vec!["say it the other side of the world"]),
            (3, vec!["the other side"]),
            (4, vec!["side to other"]),
        ];
        assert_eq!(lifted(&tracks, "other side"), [(3, 0), (2, 0), (1, 0), (4, 0)]);
    }

    #[test]
    fn a_track_is_lifted_by_its_phrase_line_though_another_ranked_first() {
        let tracks = [(1, vec!["side other", "[Chorus]", "", "on the other side of town"])];
        assert_eq!(lifted(&tracks, "other side"), [(1, 3)], "places as the lyrics were given");
    }

    #[test]
    fn a_phrase_forgives_typos_and_the_word_being_typed() {
        let tracks = [(1, vec!["side other"]), (2, vec!["from the other side now"])];
        assert_eq!(lifted(&tracks, "othr sid"), [(2, 0), (1, 0)]);
        // An earlier word is whole, in the phrase as in the line.
        assert_eq!(lifted(&tracks, "oth side"), []);
    }

    #[test]
    fn a_leading_article_is_not_part_of_the_phrase() {
        let tracks = [(1, vec!["side the other"]), (2, vec!["other side now"])];
        // Track 1 has the article, which step one ranks first; only track 2 has the phrase.
        assert_eq!(find(&LyricIndex::build(tracks.clone()), "the other side"), [(1, 0), (2, 0)]);
        assert_eq!(lifted(&tracks, "the other side"), [(2, 0), (1, 0)]);
    }

    #[test]
    fn only_the_first_tracks_are_read_back() {
        let last = i64::try_from(PHRASE_TRACKS).unwrap();
        let mut tracks: Vec<(i64, Vec<&str>)> =
            (1..last).map(|id| (id, vec!["side other"])).collect();
        tracks.push((last, vec!["other side now"]));
        assert_eq!(lifted(&tracks, "other side")[0], (last, 0));
        tracks.insert(0, (0, vec!["side other"]));
        assert_eq!(
            lifted(&tracks, "other side").last(),
            Some(&(last, 0)),
            "past the first {PHRASE_TRACKS}, not lifted"
        );
    }

    #[test]
    fn lyrics_are_read_only_for_a_phrase_and_only_what_they_give_is_lifted() {
        let index = songs();
        let mut hits = index.search("ain't", |_| true);
        let before = hits.clone();
        lift_phrases("ain't", &mut hits, |_| -> Option<Vec<&str>> { panic!("one word") });
        lift_phrases("the ain't", &mut hits, |_| -> Option<Vec<&str>> { panic!("an article") });
        lift_phrases("say ain't", &mut hits, |_| None::<Vec<&str>>);
        assert_eq!(hits, before);
    }

    /// Where the lyrics section for `query` goes among the sections of `names`, the albums with
    /// those titles, when its first match is in `line`.
    fn placed(names: &[&str], line: &str, query: &str) -> usize {
        use crate::search::{Document, Index, Kind};
        let documents = (0..)
            .zip(names)
            .filter_map(|(id, name)| Document::from_album(id, Some((*name).to_owned())));
        let sections = Index::build(documents.collect::<Vec<_>>()).search(query, &Kind::ALL, 5);
        let first = &LyricIndex::build([(1, vec![line])]).search(query, |_| true)[0];
        lyrics_place(&sections, first)
    }

    #[test]
    fn lyrics_never_outrank_an_exact_name_and_otherwise_compare_as_names_do() {
        // A name matched exactly comes first, even before a line that is the query exactly.
        assert_eq!(placed(&["Other Side"], "other side", "other side"), 1);
        assert_eq!(placed(&["The Other Side"], "other side", "other side"), 1, "an article aside");
        // Otherwise as names compare: a shorter line before a longer name, and a line with no
        // typo before a name with one.
        assert_eq!(placed(&["Other Side of the Moon"], "the other side", "other side"), 0);
        assert_eq!(placed(&["Other Sid Now"], "other side now go", "other side now"), 0);
        // When as good, after the names.
        assert_eq!(placed(&["Other Side Now"], "other side now", "other side"), 1);
        assert_eq!(placed(&[], "other side", "other side"), 0);
    }

    #[test]
    fn a_hit_reads_back_from_its_bytes() {
        let index = songs();
        for hit in
            index.search("the times", |_| true).into_iter().chain(index.search("so", |_| true))
        {
            assert_eq!(LyricHit::from_bytes(&hit.to_bytes()), Some(hit));
        }
        let bytes = index.search("so", |_| true)[0].to_bytes();
        assert_eq!(LyricHit::from_bytes(&bytes[..bytes.len() - 1]), None);
        assert_eq!(LyricHit::from_bytes(&[bytes.as_slice(), &[0]].concat()), None);
        assert_eq!(LyricHit::from_bytes(&[[2].as_slice(), &bytes[1..]].concat()), None);
    }

    #[test]
    fn lifting_says_how_many_it_lifted_and_when_it_reads() {
        let tracks =
            [(1, vec!["side other"]), (2, vec!["other side"]), (3, vec!["the other side"])];
        let index = LyricIndex::build(tracks.clone());
        let mut hits = index.search("other side", |_| true);
        let lyrics = |track| tracks.iter().find(|(id, _)| *id == track).map(|(_, l)| l.clone());
        assert_eq!(lift_phrases("other side", &mut hits, lyrics), 2);
        assert_eq!(lift_phrases("side", &mut hits, lyrics), 0);
        assert!(wants_phrases("other side"));
        assert!(!wants_phrases("side") && !wants_phrases("the side") && !wants_phrases(""));
    }

    #[test]
    fn a_word_read_back_matches_as_it_would_be_looked_up() {
        let vocabulary: Vec<String> =
            ["other", "others", "otter", "side", "sigh", "thousand"].map(str::to_owned).into();
        for query in ["other", "othr", "oth", "o", "sid", "side", "thousnad", "thousan", "x"] {
            let chars: Vec<char> = query.chars().collect();
            let read: Vec<(u32, Quality)> = (0..)
                .zip(&vocabulary)
                .filter_map(|(w, word)| Some((w, word_quality(query, &chars, word)?)))
                .collect();
            assert_eq!(read, matching_words(vocabulary.as_slice(), query), "{query}");
        }
    }
}
