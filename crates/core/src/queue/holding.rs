//! Finding the rows that read some positions of the queue's lists, such as the ones that hold
//! tracks the user hid, which a device leaves out of the queue it shows and skips as they come up
//! (`requirements/conventions.md` §8). A list never changes, so where it holds such tracks is
//! found once for each list ([`positions_holding`]), and a change to the queue only maps those
//! positions to its rows ([`QueueState::rows_reading`]), which costs what they number, not what
//! the queue does.

use std::collections::{HashMap, HashSet};

use super::spans::View;
use super::state::{ItemId, QueueState};

/// The positions of `list` that hold one of `tracks`, ascending. O(list).
pub fn positions_holding(list: &[u64], tracks: &HashSet<u64>) -> Vec<u64> {
    if tracks.is_empty() {
        return Vec::new();
    }
    list.iter().zip(0_u64..).filter(|(id, _)| tracks.contains(id)).map(|(_, at)| at).collect()
}

/// The rows of a queue that read some positions, each part's indices ascending: Up Next's, and
/// History's as [`QueueState::history_row`] numbers them, newest first.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reading {
    /// Indices of Up Next, the Manual Queue included.
    pub up_next: Vec<u64>,
    /// Indices of History.
    pub history: Vec<u64>,
}

impl QueueState {
    /// The rows that read positions `hits` of each list, by its hash, ascending: a source's
    /// rows, and a copy's that read them. O(hits × spans), and History's runs × log(hits).
    pub fn rows_reading(&self, hits: &HashMap<String, Vec<u64>>) -> Reading {
        // Each source's positions, then each copy's that read one of them.
        let mut by_key: HashMap<&str, Vec<u64>> = HashMap::new();
        for source in &self.sources {
            let positions = source.hash.as_ref().and_then(|hash| hits.get(hash));
            if let Some(positions) = positions.filter(|positions| !positions.is_empty()) {
                by_key.insert(&source.key, positions.clone());
            }
        }
        if by_key.is_empty() {
            return Reading::default();
        }
        let mut copied = Vec::new();
        for copy in &self.copies {
            let mut found = Vec::new();
            let mut at = 0;
            for span in copy.of.spans() {
                if let View::Key(key) = &span.view
                    && let Some(positions) = by_key.get(key.as_str())
                {
                    found.extend(
                        within(positions, span.start, span.len).map(|p| at + p - span.start),
                    );
                }
                at += span.len;
            }
            if !found.is_empty() {
                copied.push((copy.key.as_str(), found));
            }
        }
        by_key.extend(copied);

        let mut up_next: Vec<u64> = by_key
            .iter()
            .flat_map(|(key, positions)| {
                positions
                    .iter()
                    .filter_map(|&pos| self.find_item(&ItemId { key: (*key).to_owned(), pos }))
            })
            .collect();
        up_next.sort_unstable();

        let mut history = Vec::new();
        let mut index = 0;
        for run in self.history.iter().rev() {
            if let View::Key(key) = &run.span.view
                && let Some(positions) = by_key.get(key.as_str())
            {
                let (start, len) = (run.span.start, run.span.len);
                history
                    .extend(within(positions, start, len).map(|p| index + len - 1 - (p - start)));
            }
            index += run.span.len;
        }
        history.sort_unstable();
        Reading { up_next, history }
    }
}

/// Those of `positions`, ascending, from `start` for `len`.
fn within(positions: &[u64], start: u64, len: u64) -> impl Iterator<Item = u64> + '_ {
    let from = positions.partition_point(|&p| p < start);
    positions[from..].iter().copied().take_while(move |&p| p < start + len)
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::super::edit::{Anchor, Edit, PlayRef};
    use super::super::state::{ItemId, QueueState, Source};
    use super::*;

    /// A queue over `a` (list `ha`, tracks 10 to 15) playing its first, with `m` (list `hm`,
    /// track 12) queued by hand.
    fn queue() -> QueueState {
        let mut state = QueueState::default();
        state.start(Source::new("a", "lib", 6, None), 0).unwrap();
        state.attach_list("a", "ha".into(), Arc::from([10, 11, 12, 13, 14, 15])).unwrap();
        let source = Source::new("m", "lib", 1, None);
        state.apply(&Edit::Add { source, before: Anchor::ManualEnd }).unwrap();
        state.attach_list("m", "hm".into(), Arc::from([12])).unwrap();
        state
    }

    /// Where the lists of `state` hold `tracks`, by hash, as a device finds them.
    fn hits(state: &QueueState, tracks: &[u64]) -> HashMap<String, Vec<u64>> {
        let tracks: HashSet<u64> = tracks.iter().copied().collect();
        state
            .sources
            .iter()
            .filter_map(|s| Some((s.hash.clone()?, positions_holding(s.tracks()?, &tracks))))
            .collect()
    }

    #[test]
    fn finds_where_a_list_holds_the_tracks() {
        let list = [10, 11, 12, 11];
        assert_eq!(positions_holding(&list, &[11, 99].into_iter().collect()), [1, 3]);
        assert_eq!(positions_holding(&list, &HashSet::new()), Vec::<u64>::new());
    }

    #[test]
    fn finds_the_rows_that_read_them_in_up_next_and_history() {
        let mut state = queue();
        // Up Next: m.0 (12), then a.1 to a.5 (11 to 15).
        let reading = |state: &QueueState, tracks: &[u64]| state.rows_reading(&hits(state, tracks));
        assert_eq!(reading(&state, &[12]).up_next, [0, 2]);
        assert_eq!(reading(&state, &[15, 11]).up_next, [1, 5]);
        assert_eq!(reading(&state, &[99]), Reading::default());
        for _ in 0..3 {
            state.apply(&Edit::Next).unwrap();
        }
        // Played: a.0, m.0, a.1, newest first; a.2 (12) plays now, so neither part has it.
        assert_eq!(reading(&state, &[12]), Reading { up_next: vec![], history: vec![1] });
        assert_eq!(reading(&state, &[10, 11]).history, [0, 2]);

        // A copy of a played item reads its source: a.0 again, at the front of Up Next.
        let play = PlayRef { play: 0, item: ItemId { key: "a".into(), pos: 0 } };
        state.apply(&Edit::Requeue { play, key: "c".into() }).unwrap();
        assert_eq!(reading(&state, &[10]), Reading { up_next: vec![0], history: vec![] });
    }

    /// Shuffled, a context row reads the base item the permutation maps it to, which
    /// `rows_reading` finds back through `order_row_of`: for several seeds, and with the Manual
    /// Queue ahead, a copy, and plays behind, its rows are exactly those whose tracks Up Next and
    /// History show as the ones asked for, as `upNextRows` and `historyRows` read them.
    #[test]
    fn finds_the_rows_of_a_shuffled_queue_as_they_read() {
        for seed in [1, 7, 4242, u32::MAX] {
            let mut state = QueueState::default();
            state.start(Source::new("a", "lib", 40, None), 0).unwrap();
            state.attach_list("a", "ha".into(), (100..140).collect()).unwrap();
            let source = Source::new("m", "lib", 3, None);
            state.apply(&Edit::Add { source, before: Anchor::ManualEnd }).unwrap();
            state.attach_list("m", "hm".into(), Arc::from([105, 130, 199])).unwrap();
            state.apply(&Edit::Shuffle { on: true, seed }).unwrap();
            for _ in 0..6 {
                state.apply(&Edit::Next).unwrap();
            }
            let play = PlayRef { play: 1, item: state.history_row(1).unwrap().row.item };
            state.apply(&Edit::Requeue { play, key: "c".into() }).unwrap();

            let picks: [Vec<u64>; 4] = [
                vec![105],
                vec![100, 101, 102, 103],
                (100..140).step_by(3).collect(),
                vec![199, 139],
            ];
            for tracks in picks {
                let reading = state.rows_reading(&hits(&state, &tracks));
                let holds = |track: Option<u64>| track.is_some_and(|t| tracks.contains(&t));
                let up_next: Vec<u64> = (0..state.up_next_len())
                    .filter(|&i| holds(state.up_next_row(i).and_then(|row| row.track)))
                    .collect();
                let history: Vec<u64> = (0..state.history_len())
                    .filter(|&i| holds(state.history_row(i).and_then(|play| play.row.track)))
                    .collect();
                assert_eq!(reading, Reading { up_next, history }, "seed {seed}, {tracks:?}");
            }
        }
    }

    #[test]
    #[ignore = "timing run: cargo test -p jewelcase-core --release -- --ignored --nocapture"]
    fn finding_hidden_rows_of_500k_is_fast() {
        let mut state = QueueState::default();
        state.start(Source::new("big", "lib", 500_000, None), 0).unwrap();
        state.attach_list("big", "h".into(), (0..500_000).collect()).unwrap();
        let hidden: HashSet<u64> = (0..100).map(|i| i * 4_999).collect();

        let started = Instant::now();
        let positions = positions_holding(state.sources[0].tracks().unwrap(), &hidden);
        let scanned = started.elapsed();
        let hits = HashMap::from([("h".to_owned(), positions)]);
        let started = Instant::now();
        let reading = state.rows_reading(&hits);
        let mapped = started.elapsed();
        println!(
            "scan a 500k list, once: {scanned:?}; map 100 hits to rows, each change: {mapped:?}"
        );
        assert_eq!(reading.up_next.len(), 99);
        let bound = if cfg!(debug_assertions) {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(1)
        };
        assert!(mapped < bound, "mapping took {mapped:?}");
    }
}
