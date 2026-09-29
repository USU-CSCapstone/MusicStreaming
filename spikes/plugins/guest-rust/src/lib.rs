//! The spike's test plugin in Rust: the best case for guest cost.

use std::cell::Cell;
use std::hint::black_box;

wit_bindgen::generate!({ path: "../wit", world: "plugin" });

use jewelcase::spike::{library, state};

struct Plugin;

thread_local! {
    static COUNTER: Cell<u64> = const { Cell::new(0) };
}

impl Guest for Plugin {
    fn noop() {}

    fn echo(s: String) -> String {
        s
    }

    fn scan_titles(needle: String, batch: u32) -> u32 {
        let mut found = 0;
        let mut offset = 0;
        loop {
            let page = library::tracks(offset, batch);
            if page.is_empty() {
                return found;
            }
            found += page.iter().filter(|t| t.title.contains(&needle)).count() as u32;
            offset += page.len() as u32;
        }
    }

    fn counter() -> u64 {
        COUNTER.with(|c| {
            c.set(c.get() + 1);
            c.get()
        })
    }

    fn persisted_counter() -> u64 {
        let n = state::get("counter")
            .and_then(|b| b.try_into().ok())
            .map(u64::from_le_bytes)
            .unwrap_or(0)
            + 1;
        state::set("counter", &n.to_le_bytes());
        n
    }

    fn spin() {
        let mut x = 0u64;
        loop {
            x = black_box(x.wrapping_add(1));
        }
    }

    fn crash() {
        panic!("plugin crashed on purpose");
    }

    fn hog(mb: u32) -> u32 {
        let mut held: Vec<Vec<u8>> = Vec::new();
        for _ in 0..mb {
            // A refused allocation aborts, which traps: the host's limiter is what stops this.
            held.push(vec![1u8; 1 << 20]);
        }
        black_box(&held);
        held.len() as u32
    }
}

export!(Plugin);
