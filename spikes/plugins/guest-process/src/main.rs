//! The spike's test plugin as a native child process: the out-of-process baseline.
//! The same operations as the Wasm guests, over stdin/stdout.

mod protocol;

use std::hint::black_box;
use std::io::{self, BufReader, BufWriter, Read, Write};

use protocol::{Call, ToGuest, ToHost};
use serde_json::json;

struct Conn {
    input: BufReader<io::Stdin>,
    output: BufWriter<io::Stdout>,
}

impl Conn {
    fn send(&mut self, msg: &ToHost) {
        let body = serde_json::to_vec(msg).unwrap();
        self.output.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
        self.output.write_all(&body).unwrap();
        self.output.flush().unwrap();
    }

    fn recv(&mut self) -> Option<ToGuest> {
        let mut len = [0u8; 4];
        self.input.read_exact(&mut len).ok()?;
        let mut body = vec![0u8; u32::from_le_bytes(len) as usize];
        self.input.read_exact(&mut body).ok()?;
        Some(serde_json::from_slice(&body).unwrap())
    }

    fn tracks(&mut self, offset: u32, limit: u32) -> Vec<protocol::Track> {
        self.send(&ToHost::Tracks { offset, limit });
        match self.recv() {
            Some(ToGuest::Tracks { page }) => page,
            other => panic!("expected tracks, got {other:?}"),
        }
    }
}

fn main() {
    let mut conn = Conn {
        input: BufReader::new(io::stdin()),
        output: BufWriter::new(io::stdout()),
    };
    let mut counter = 0u64;
    while let Some(msg) = conn.recv() {
        let ToGuest::Call(call) = msg else {
            panic!("expected a call");
        };
        let value = match call {
            Call::Noop => json!(null),
            Call::Echo { s } => json!(s),
            Call::ScanTitles { needle, batch } => {
                let (mut found, mut offset) = (0u32, 0u32);
                loop {
                    let page = conn.tracks(offset, batch);
                    if page.is_empty() {
                        break;
                    }
                    found += page.iter().filter(|t| t.title.contains(&needle)).count() as u32;
                    offset += page.len() as u32;
                }
                json!(found)
            }
            Call::Counter => {
                counter += 1;
                json!(counter)
            }
            Call::PersistedCounter => {
                conn.send(&ToHost::StateGet { key: "counter".into() });
                let prev = match conn.recv() {
                    Some(ToGuest::State { value }) => value
                        .and_then(|b| b.try_into().ok())
                        .map(u64::from_le_bytes)
                        .unwrap_or(0),
                    other => panic!("expected state, got {other:?}"),
                };
                let n = prev + 1;
                conn.send(&ToHost::StateSet {
                    key: "counter".into(),
                    value: n.to_le_bytes().to_vec(),
                });
                conn.recv();
                json!(n)
            }
            Call::Spin => {
                let mut x = 0u64;
                loop {
                    x = black_box(x.wrapping_add(1));
                }
            }
            Call::Crash => panic!("plugin crashed on purpose"),
            Call::Hog { mb } => {
                let held: Vec<Vec<u8>> = (0..mb).map(|_| vec![1u8; 1 << 20]).collect();
                black_box(&held);
                json!(held.len())
            }
        };
        conn.send(&ToHost::Done { value });
    }
}
