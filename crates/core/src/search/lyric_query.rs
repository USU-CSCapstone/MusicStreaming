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
//! lines that hold the query as a phrase are lifted afterwards, from their text.

use super::lyrics::LyricIndex;
use super::query::{MAX_QUERY_WORDS, Quality, leading_article, matching_words};
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
        let mut query_words = words(query);
        query_words.truncate(MAX_QUERY_WORDS);
        let Some(last) = query_words.len().checked_sub(1) else {
            return Vec::new();
        };
        let optional = leading_article(&query_words, |word| ARTICLES.contains(&word.as_str()));

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
            let (typos, prefixes) = qualities.iter().fold((0, 0), |(typos, prefixes), &q| {
                (typos + u32::from((q - 1) / 2), prefixes + u32::from((q - 1) % 2))
            });
            let score = LyricScore {
                typos,
                skipped: optional > 0 && article.is_none(),
                prefixes,
                extra_words: u32::from(self.length(line)).saturating_sub(qualities.len() as u32),
            };
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
    fn what_keep_leaves_out_is_not_found() {
        let index = songs();
        assert_eq!(index.search("ain't", |track| track != 1).len(), 1);
        assert!(index.search("", |_| true).is_empty());
        assert!(index.search("?!", |_| true).is_empty());
        assert!(LyricIndex::default().search("so", |_| true).is_empty());
    }
}
