//! The scoped data layer plugins read through, over a synthetic catalog.
//!
//! The catalog lives in memory, not SQLite: the spike measures what crossing the
//! plugin boundary costs, and a database's paging costs would only blur that.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

pub use crate::protocol::Track;

/// The library at full-machine scale (`requirements/performance.md` §1).
pub const BIG: u64 = 1;
/// A second, smaller library: scoping tests, and one-track-per-call scans.
pub const SMALL: u64 = 2;

const WORDS: &[&str] = &[
    "Love", "Night", "Blue", "River", "Fire", "Dream", "Heart", "Rain", "Gold", "Summer", "City",
    "Ghost", "Light", "Road", "Stone", "Wild", "Echo", "Sugar", "Silver", "Morning",
];

pub struct Catalog {
    libraries: HashMap<u64, Arc<Vec<Track>>>,
}

impl Catalog {
    /// Deterministic, so every subject scans the same titles and must find the same count.
    pub fn generate(big: usize, small: usize) -> Catalog {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut make = |n: usize| {
            Arc::new(
                (0..n)
                    .map(|_| {
                        let r = next();
                        let words = 1 + (r % 3) as usize;
                        let title = (0..words)
                            .map(|i| WORDS[((r >> (8 + i * 8)) % WORDS.len() as u64) as usize])
                            .collect::<Vec<_>>()
                            .join(" ");
                        Track {
                            id: next() >> 1,
                            title,
                            artist: (r % 7 != 0).then(|| format!("Artist {}", r % 5000)),
                            duration_us: 120_000_000 + r % 240_000_000,
                        }
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let libraries = HashMap::from([(BIG, make(big)), (SMALL, make(small))]);
        Catalog { libraries }
    }

    pub fn tracks(&self, library: u64) -> Arc<Vec<Track>> {
        self.libraries.get(&library).cloned().unwrap_or_default()
    }
}

/// One plugin's view of one library. Every read goes through a scope, and the
/// plugin never supplies the library itself (`design/general.md` §6).
#[derive(Clone)]
pub struct Scope {
    pub plugin: String,
    tracks: Arc<Vec<Track>>,
    state: Arc<StateStore>,
    pub probe: Arc<Probe>,
}

impl Scope {
    pub fn new(catalog: &Catalog, library: u64, plugin: &str, state: Arc<StateStore>) -> Scope {
        Scope {
            plugin: plugin.to_owned(),
            tracks: catalog.tracks(library),
            state,
            probe: Arc::default(),
        }
    }

    pub fn page(&self, offset: u32, limit: u32) -> Vec<Track> {
        self.probe.hit();
        let start = (offset as usize).min(self.tracks.len());
        let end = start.saturating_add(limit as usize).min(self.tracks.len());
        self.tracks[start..end].to_vec()
    }

    pub fn get(&self, id: u64) -> Option<Track> {
        self.tracks.iter().find(|t| t.id == id).cloned()
    }

    /// Borrowed, for the native floor: what the host pays when nothing crosses a boundary.
    pub fn all(&self) -> &[Track] {
        &self.tracks
    }

    pub fn state_get(&self, key: &str) -> Option<Vec<u8>> {
        self.state.0.lock().unwrap().get(&(self.plugin.clone(), key.to_owned())).cloned()
    }

    pub fn state_set(&self, key: String, value: Vec<u8>) {
        self.state.0.lock().unwrap().insert((self.plugin.clone(), key), value);
    }
}

/// Plugin state that outlives any one instance (`requirements/plugins.md` §2.1).
#[derive(Default)]
pub struct StateStore(Mutex<HashMap<(String, String), Vec<u8>>>);

/// When a plugin last read library data: how E7 sees a paused plugin stop.
#[derive(Default)]
pub struct Probe {
    pub calls: AtomicU64,
    last_ns: AtomicU64,
}

fn epoch() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

pub fn now_ns() -> u64 {
    epoch().elapsed().as_nanos() as u64
}

impl Probe {
    fn hit(&self) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.last_ns.store(now_ns(), Ordering::Relaxed);
    }

    pub fn last_ns(&self) -> u64 {
        self.last_ns.load(Ordering::Relaxed)
    }
}
