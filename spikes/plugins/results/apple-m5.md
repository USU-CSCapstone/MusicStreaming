
## Plugin spike results

Machine: Apple M5, 10 cores, macos aarch64. Host runtime: tokio, 4 worker threads (Pi 4-like). Epoch tick: 250 µs.


### E1 call overhead

| Subject | Measure | Result | Bar | |
|---|---|---|---|---|
| native fn | echo p50 / p99 | 0.04 µs / 0.04 µs |  |  |
| wasm · Rust | noop p50 / p99 | 0.58 µs / 0.75 µs | p99 ≤ 10 µs | ✅ |
| wasm · Rust | echo 64 B p50 / p99 | 0.67 µs / 0.88 µs | p99 ≤ 10 µs | ✅ |
| wasm · JS | noop p50 / p99 | 3.67 µs / 11 µs | p99 ≤ 10 µs | ❌ |
| wasm · JS | echo 64 B p50 / p99 | 9.96 µs / 13 µs | p99 ≤ 10 µs | ❌ |
| wasm · JS (AOT) | noop p50 / p99 | 3.83 µs / 5.21 µs | p99 ≤ 10 µs | ✅ |
| wasm · JS (AOT) | echo 64 B p50 / p99 | 10 µs / 14 µs | p99 ≤ 10 µs | ❌ |
| process · Rust | noop p50 / p99 | 9.25 µs / 14 µs |  |  |
| process · Rust | echo 64 B p50 / p99 | 9.50 µs / 14 µs |  |  |

### E2 warm & footprint

| Subject | Measure | Result | Bar | |
|---|---|---|---|---|
| wasm · Rust | component size | 72 KB |  |  |
| wasm · Rust | compile (once per version) | 9.06 ms |  |  |
| wasm · Rust | load precompiled | 0.13 ms |  |  |
| wasm · Rust | instantiate, on-demand (p50 / p99) | 11 µs / 15 µs | < 100 µs | ✅ |
| wasm · Rust | instantiate, pooling (p50 / p99) | 6.62 µs / 10 µs | < 100 µs | ✅ |
| wasm · JS | component size | 12.0 MB |  |  |
| wasm · JS | compile (once per version) | 852 ms |  |  |
| wasm · JS | load precompiled | 3.55 ms |  |  |
| wasm · JS | instantiate, on-demand (p50 / p99) | 402 µs / 462 µs |  |  |
| wasm · JS | instantiate, pooling (p50 / p99) | 388 µs / 428 µs |  |  |
| wasm · JS (AOT) | component size | 15.2 MB |  |  |
| wasm · JS (AOT) | compile (once per version) | 1004 ms |  |  |
| wasm · JS (AOT) | load precompiled | 4.32 ms |  |  |
| wasm · JS (AOT) | instantiate, on-demand (p50 / p99) | 432 µs / 508 µs |  |  |
| wasm · JS (AOT) | instantiate, pooling (p50 / p99) | 431 µs / 499 µs |  |  |
| wasm · Rust | idle RSS | 344 KB for 10, 2.0 MB for 50 (41 KB each) | ≤ 5 MB per plugin | ✅ |
| wasm · JS | idle RSS | 84.9 MB for 10, 424.7 MB for 50 (8.5 MB each) | ≤ 5 MB per plugin | ❌ |
| wasm · JS (AOT) | idle RSS | 91.4 MB for 10, 457.1 MB for 50 (9.1 MB each) | ≤ 5 MB per plugin | ❌ |
| process · Rust | idle RSS | 16.6 MB for 10, 83.1 MB for 50 (1.7 MB each) | ≤ 5 MB per plugin | ✅ |

### E3 batched data access

| Subject | Measure | Result | Bar | |
|---|---|---|---|---|
| native, borrowed | 500k tracks | 4.13 ms |  |  |
| native, paged | batch 1000, 500k tracks | 24 ms (48 ns/track) |  |  |
| wasm · Rust | batch 1000, 500k tracks | 77 ms (153 ns/track, 3.2× paged) | ≤ 2× native paged | ❌ |
| wasm · JS | batch 1000, 500k tracks | 1647 ms (3294 ns/track, 68.3× paged) | ≤ 2× native paged | ❌ |
| wasm · JS (AOT) | batch 1000, 500k tracks | 1339 ms (2678 ns/track, 55.5× paged) | ≤ 2× native paged | ❌ |
| process · Rust | batch 1000, 500k tracks | 164 ms (327 ns/track, 6.8× paged) | ≤ 2× native paged | ❌ |
| native, paged | batch 100, 500k tracks | 23 ms (47 ns/track) |  |  |
| wasm · Rust | batch 100, 500k tracks | 78 ms (155 ns/track, 3.3× paged) |  |  |
| wasm · JS | batch 100, 500k tracks | 1679 ms (3358 ns/track, 72.0× paged) |  |  |
| wasm · JS (AOT) | batch 100, 500k tracks | 1346 ms (2693 ns/track, 57.7× paged) |  |  |
| process · Rust | batch 100, 500k tracks | 224 ms (448 ns/track, 9.6× paged) |  |  |
| native, paged | batch 1, 20k tracks | 1.27 ms (63 ns/track) |  |  |
| wasm · Rust | batch 1, 20k tracks | 5.67 ms (284 ns/track, 4.5× paged) |  |  |
| wasm · JS | batch 1, 20k tracks | 122 ms (6116 ns/track, 96.5× paged) |  |  |
| wasm · JS (AOT) | batch 1, 20k tracks | 95 ms (4735 ns/track, 74.7× paged) |  |  |
| process · Rust | batch 1, 20k tracks | 197 ms (9867 ns/track, 155.6× paged) |  |  |

### E4 hangs are bounded

| Subject | Measure | Result | Bar | |
|---|---|---|---|---|
| wasm · Rust | stopped after (50 ms budget) | 50 ms | ≤ deadline + 5 ms | ✅ |
| wasm · Rust | back to a working instance | 0.05 ms |  |  |
| wasm · JS | stopped after (50 ms budget) | 50 ms | ≤ deadline + 5 ms | ✅ |
| wasm · JS | back to a working instance | 0.55 ms |  |  |
| wasm · JS (AOT) | stopped after (50 ms budget) | 50 ms | ≤ deadline + 5 ms | ✅ |
| wasm · JS (AOT) | back to a working instance | 0.57 ms |  |  |
| process · Rust | stopped after (50 ms budget) | 53 ms | ≤ deadline + 5 ms | ✅ |
| process · Rust | back to a working instance | 1.12 ms |  |  |

### E5 crashes & memory

| Subject | Measure | Result | Bar | |
|---|---|---|---|---|
| wasm · Rust | crash | contained; fresh instance works | contained | ✅ |
| wasm · Rust | allocate 1 GiB (128 MiB limit) | refused at 128 MiB; host grew 132.2 MB, 0 KB after drop | refused | ✅ |
| wasm · JS | crash | contained; fresh instance works | contained | ✅ |
| wasm · JS | allocate 1 GiB (128 MiB limit) | refused at 128 MiB; host grew 132.2 MB, 0 KB after drop | refused | ✅ |
| wasm · JS (AOT) | crash | contained; fresh instance works | contained | ✅ |
| wasm · JS (AOT) | allocate 1 GiB (128 MiB limit) | refused at 128 MiB; host grew 132.2 MB, 0 KB after drop | refused | ✅ |
| process · Rust | crash | contained; fresh instance works | contained | ✅ |
| process · Rust | allocate 1 GiB (128 MiB limit) | allocated 1024 MiB unchecked | refused | ❌ |

### E6 no plugin blocks another

| Subject | Measure | Result | Bar | |
|---|---|---|---|---|
| wasm · Rust | all well-behaved | p50 33 µs · p99 62 µs · max 85 µs |  |  |
| wasm · Rust | one plugin spinning | p50 8.79 µs · p99 22 µs · max 55 µs |  |  |
| wasm · Rust | 4 spinning (every worker) | p50 404 µs · p99 1.47 ms · max 1.90 ms | p99 ≤ 2 ms (a tick + margin) | ✅ |
| wasm · Rust | slowdown from one spinner | 0.3× | ≤ 2× | ✅ |
| process · Rust | all well-behaved | p50 64 µs · p99 112 µs · max 1.29 ms |  |  |
| process · Rust | one plugin spinning | p50 36 µs · p99 91 µs · max 1.87 ms |  |  |
| process · Rust | 4 spinning (every worker) | p50 28 µs · p99 46 µs · max 639 µs |  |  |
| process · Rust | slowdown from one spinner | 0.8× | ≤ 2× | ✅ |

### E7 background work yields

| Subject | Measure | Result | Bar | |
|---|---|---|---|---|
| wasm · Rust | parked after pause | 273 µs | ≤ 2 ms | ✅ |
| wasm · JS | parked after pause | before its next read | ≤ 2 ms | ✅ |
| wasm · JS (AOT) | parked after pause | before its next read | ≤ 2 ms | ✅ |
| process · Rust | parked after pause (SIGSTOP) | immediate | ≤ 2 ms | ✅ |

### E8 state & scoping

| Subject | Measure | Result | Bar | |
|---|---|---|---|---|
| wasm · Rust | state and scope | warm [1, 2, 3]; persisted 1, 2 → reinstantiate → 3; libraries see 20000 / 500000 | exact | ✅ |
| wasm · JS | state and scope | warm [1, 2, 3]; persisted 1, 2 → reinstantiate → 3; libraries see 20000 / 500000 | exact | ✅ |
| wasm · JS (AOT) | state and scope | warm [1, 2, 3]; persisted 1, 2 → reinstantiate → 3; libraries see 20000 / 500000 | exact | ✅ |
| process · Rust | state and scope | warm [1, 2, 3]; persisted 1, 2 → reinstantiate → 3; libraries see 20000 / 500000 | exact | ✅ |

All correctness checks passed.
