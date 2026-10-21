//! The background workers' threads run at low priority. The only test in its binary, so no
//! other test's threads come and go while it reads them.

use std::sync::Arc;
use std::time::Duration;

use jewelcase_ffmpeg::{Config, Ffmpeg};
use jewelcase_scanner::analysis::Analyzer;
use jewelcase_scanner::placeholders::Placeholders;
use jewelcase_scanner::{Governor, MemoryStore};

/// The nice value in a `/proc/.../stat` line: its 19th field, after the name in parentheses,
/// which may hold spaces.
fn nice_in(stat: &str) -> i32 {
    stat.rsplit_once(") ").unwrap().1.split(' ').nth(16).unwrap().parse().unwrap()
}

/// The nice values of this process's threads named `name`, as Linux shortens it, to 15 bytes.
fn nice_of(name: &str) -> Vec<i32> {
    let name = &name[..15];
    let tasks = std::fs::read_dir("/proc/self/task").unwrap();
    let stats = tasks.map(|task| std::fs::read_to_string(task.unwrap().path().join("stat")));
    let named = stats.map(Result::unwrap).filter(|stat| stat.contains(&format!("({name})")));
    named.map(|stat| nice_in(&stat)).collect()
}

#[test]
fn analysis_and_placeholder_threads_run_at_low_priority() {
    let own = nice_in(&std::fs::read_to_string("/proc/thread-self/stat").unwrap());
    let lowered = (own + 10).min(19);
    let ffmpeg = Ffmpeg::new(Config::default());
    let store = Arc::new(MemoryStore::new());
    let analyzer =
        Arc::new(Analyzer::new(ffmpeg.clone(), store.clone(), Arc::new(Governor::new())));
    let placeholders = Arc::new(Placeholders::new(ffmpeg, store));
    let idle = Duration::from_secs(60);
    let workers = (analyzer.start(1, idle), placeholders.start(1, idle));
    for name in ["scanner-analysis", "scanner-placeholders"] {
        let lowered_soon = (0..500).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            nice_of(name) == [lowered]
        });
        assert!(lowered_soon, "{name}: {:?}, not [{lowered}]", nice_of(name));
    }
    workers.0.stop();
    workers.1.stop();
}
