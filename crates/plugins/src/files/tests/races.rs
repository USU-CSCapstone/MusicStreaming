//! Calls racing a folder that changes under them, and each other.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread;

use rustix::fs::{CWD, RenameFlags, renameat_with};

use super::*;

/// How many times a racing test calls the host.
const ROUNDS: usize = 2000;

/// Sets its flag when dropped, so a racing thread watching it stops even if the test panics.
struct StopOnDrop<'a>(&'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    /// Tells the racing thread to stop.
    fn drop(&mut self) {
        self.0.store(true, Relaxed);
    }
}

/// While a folder within the library and a link out of it swap places as fast as they can, no
/// listing, read, or write through that name reaches outside.
#[test]
fn a_link_swapped_in_mid_call_never_redirects_it() {
    let (temp, root) = library();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret"), b"host file").unwrap();
    fs::write(outside.join("host-only"), b"").unwrap();
    fs::create_dir(root.join("flip")).unwrap();
    fs::write(root.join("flip/secret"), b"library").unwrap();
    symlink(&outside, root.join("link")).unwrap();
    let stop = AtomicBool::new(false);
    let (mut leaked, mut reached) = (Vec::new(), [0; 3]);
    thread::scope(|s| {
        s.spawn(|| {
            let (flip, link) = (root.join("flip"), root.join("link"));
            while !stop.load(Relaxed) {
                renameat_with(CWD, &flip, CWD, &link, RenameFlags::EXCHANGE).unwrap();
            }
        });
        let _stopping = StopOnDrop(&stop);
        for round in 0..ROUNDS {
            if let Ok(listed) = entries(&root, "flip", None) {
                reached[0] += 1;
                if listed.iter().any(|e| e.name == "host-only") {
                    leaked.push(format!("listed in round {round}"));
                }
            }
            if let Ok(contents) = read(&root, "flip/secret", 0, 64) {
                reached[1] += 1;
                if contents != b"library" {
                    leaked.push(format!("read in round {round}"));
                }
            }
            reached[2] += usize::from(write(&root, &format!("flip/{round}"), b"x", Create).is_ok());
        }
    });
    assert_eq!(leaked.first(), None, "{} calls reached outside the library", leaked.len());
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 2, "nothing was written outside");
    assert_eq!(fs::read(outside.join("secret")).unwrap(), b"host file");
    assert!(reached.iter().all(|&n| n > 0), "the folder was reached between swaps: {reached:?}");
}

/// Calls replacing one file at once each stage their own copy, so every one succeeds, and the
/// file ends whole as one of them wrote it.
#[test]
fn concurrent_writes_stage_apart() {
    let (_temp, root) = library();
    write(&root, "Album/01.lrc", b"first", Create).unwrap();
    let contents: Vec<Vec<u8>> = (0..4u8).map(|n| vec![n; 4096]).collect();
    thread::scope(|s| {
        for mine in &contents {
            s.spawn(|| {
                for _ in 0..ROUNDS / 10 {
                    write(&root, "Album/01.lrc", mine, Replace).unwrap();
                }
            });
        }
    });
    assert!(contents.contains(&fs::read(root.join("Album/01.lrc")).unwrap()));
    assert_eq!(names(&root, "Album", None), ["01.flac", "01.lrc"], "no staged files left behind");
}

/// Calls creating one file at once make it once: one succeeds, and every other fails, leaving
/// the winner's file as it wrote it.
#[test]
fn concurrent_creates_make_one_file() {
    let (_temp, root) = library();
    for round in 0..ROUNDS / 100 {
        let (root, path) = (&root, &format!("Album/{round}.lrc"));
        let made: Vec<_> = thread::scope(|s| {
            let calls: Vec<_> = (0..4u8)
                .map(|n| s.spawn(move || write(root, path, &[n; 4096], Create).map(|_| n)))
                .collect();
            calls.into_iter().map(|call| call.join().unwrap()).collect()
        });
        let winners: Vec<u8> = made.iter().filter_map(|m| m.as_ref().ok().copied()).collect();
        assert_eq!(winners.len(), 1, "round {round}: {made:?}");
        let mut lost = made.into_iter().filter_map(Result::err);
        assert!(lost.all(|e| matches!(e, Error::Failed(why) if why.contains("already exists"))));
        assert_eq!(fs::read(root.join(path)).unwrap(), [winners[0]; 4096]);
    }
}
