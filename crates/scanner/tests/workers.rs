//! The background pool's threads run at low priority. The only test in its binary, so no other
//! test's threads come and go while it reads them.

use std::sync::Arc;
use std::time::Duration;

use jewelcase_ffmpeg::{Config, Ffmpeg};
use jewelcase_scanner::testing::BareStore;
use jewelcase_scanner::{Analyzer, Placeholders, Pool};

/// The nice value in a `/proc/.../stat` line: its 19th field, after the name in parentheses,
/// which may hold spaces.
fn nice_in(stat: &str) -> i32 {
    stat.rsplit_once(") ").unwrap().1.split(' ').nth(16).unwrap().parse().unwrap()
}

/// The nice values of this process's threads named `name`, which must fit Linux's 15 bytes.
fn nice_of(name: &str) -> Vec<i32> {
    let tasks = std::fs::read_dir("/proc/self/task").unwrap();
    let stats = tasks.map(|task| std::fs::read_to_string(task.unwrap().path().join("stat")));
    let named = stats.map(Result::unwrap).filter(|stat| stat.contains(&format!("({name})")));
    named.map(|stat| nice_in(&stat)).collect()
}

#[test]
fn the_pools_threads_run_at_low_priority() {
    let own = nice_in(&std::fs::read_to_string("/proc/thread-self/stat").unwrap());
    let lowered = (own + 10).min(19);
    let ffmpeg = Ffmpeg::new(Config::default());
    let store = Arc::new(BareStore::default());
    let analyzer = Arc::new(Analyzer::new(ffmpeg.clone(), store.clone()));
    let placeholders = Arc::new(Placeholders::new(ffmpeg, store));
    let pool = Pool::start(vec![analyzer.job(1), placeholders.job(1)], 3).unwrap();
    let lowered_soon = (0..500).any(|_| {
        std::thread::sleep(Duration::from_millis(10));
        nice_of("scanner-decode") == [lowered; 3]
    });
    assert!(lowered_soon, "{:?}, not [{lowered}; 3]", nice_of("scanner-decode"));
    drop(pool);
}
