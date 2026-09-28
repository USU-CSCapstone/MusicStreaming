//! The plugin execution-model spike (`design/general.md` §8, §11 #4).
//!
//! Measures WebAssembly components hosted in-process by Wasmtime (Rust and JS guests)
//! against an out-of-process baseline, on the constraints in `requirements/plugins.md`
//! §2 and §11. Performance bars are proposals, reported rather than asserted;
//! containment and scoping are asserted, and any failure exits non-zero.
//!
//!     plugin-spike [all|calls|instantiate|data|hang|crash|concurrency|yield|state] [--json FILE]

mod library;
#[path = "../../guest-process/src/protocol.rs"]
mod protocol;
mod process;
mod stats;
mod wasm;

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::Serialize;

use library::{BIG, Catalog, SMALL, Scope, StateStore};
use process::ProcessPlugin;
use protocol::Call;
use stats::{mb, ms, summarize, us};
use wasm::{Instance, Loaded, Opts, Pause, Runtime};

const BIG_TRACKS: usize = 500_000;
const SMALL_TRACKS: usize = 20_000;
const NEEDLE: &str = "Love";
const LONG: Duration = Duration::from_secs(120);

fn guest_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Rust,
    Js,
    JsAot,
    Process,
}

impl Kind {
    const ALL: [Kind; 4] = [Kind::Rust, Kind::Js, Kind::JsAot, Kind::Process];
    const WASM: [Kind; 3] = [Kind::Rust, Kind::Js, Kind::JsAot];

    fn label(self) -> &'static str {
        match self {
            Kind::Rust => "wasm · Rust",
            Kind::Js => "wasm · JS",
            Kind::JsAot => "wasm · JS (AOT)",
            Kind::Process => "process · Rust",
        }
    }

    fn file(self) -> Option<&'static str> {
        match self {
            Kind::Rust => Some("target/wasm32-wasip2/release/guest_rust.wasm"),
            Kind::Js => Some("guest-js/guest_js.wasm"),
            Kind::JsAot => Some("guest-js/guest_js_aot.wasm"),
            Kind::Process => None,
        }
    }
}

/// One plugin instance of any kind, behind the same calls.
enum Guest {
    Wasm(Instance),
    Process(ProcessPlugin),
}

impl Guest {
    async fn noop(&mut self) -> Result<()> {
        match self {
            Guest::Wasm(i) => i.noop().await,
            Guest::Process(p) => p.noop().await,
        }
    }

    async fn echo(&mut self, s: &str) -> Result<String> {
        match self {
            Guest::Wasm(i) => i.echo(s).await,
            Guest::Process(p) => p.echo(s).await,
        }
    }

    async fn scan(&mut self, batch: u32, needle: &str, budget: Duration) -> Result<u32> {
        match self {
            Guest::Wasm(i) => i.scan_titles(needle, batch, budget).await,
            Guest::Process(p) => {
                let call = Call::ScanTitles { needle: needle.to_owned(), batch };
                p.number(call, budget).await.map(|n| n as u32)
            }
        }
    }

    async fn counter(&mut self) -> Result<u64> {
        match self {
            Guest::Wasm(i) => i.counter().await,
            Guest::Process(p) => p.number(Call::Counter, LONG).await,
        }
    }

    async fn persisted(&mut self) -> Result<u64> {
        match self {
            Guest::Wasm(i) => i.persisted_counter().await,
            Guest::Process(p) => p.number(Call::PersistedCounter, LONG).await,
        }
    }

    async fn spin(&mut self, budget: Duration) -> Result<()> {
        match self {
            Guest::Wasm(i) => i.spin(budget).await,
            Guest::Process(p) => p.call_within(Call::Spin, budget).await.map(|_| ()),
        }
    }

    async fn crash(&mut self) -> Result<()> {
        match self {
            Guest::Wasm(i) => i.crash().await,
            Guest::Process(p) => p.call_within(Call::Crash, LONG).await.map(|_| ()),
        }
    }

    async fn hog(&mut self, megabytes: u32) -> Result<u32> {
        match self {
            Guest::Wasm(i) => i.hog(megabytes).await,
            Guest::Process(p) => p.number(Call::Hog { mb: megabytes }, LONG).await.map(|n| n as u32),
        }
    }
}

// ───────────────────────────── Report ─────────────────────────────

#[derive(Serialize, Clone, Copy, PartialEq)]
enum Verdict {
    Pass,
    Miss,
    Info,
}

#[derive(Serialize)]
struct Row {
    experiment: &'static str,
    subject: String,
    metric: String,
    value: String,
    /// The value in base units (seconds, bytes, or a count), for the JSON record.
    raw: f64,
    bar: String,
    verdict: Verdict,
}

struct Harness {
    rt: Runtime,
    loaded: Vec<(Kind, Loaded)>,
    catalog: Catalog,
    state: Arc<StateStore>,
    rows: Vec<Row>,
    failures: Vec<String>,
}

impl Harness {
    fn row(&mut self, experiment: &'static str, subject: &str, metric: &str, value: String, raw: f64) {
        self.rows.push(Row {
            experiment,
            subject: subject.into(),
            metric: metric.into(),
            value,
            raw,
            bar: String::new(),
            verdict: Verdict::Info,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn judged(
        &mut self,
        experiment: &'static str,
        subject: &str,
        metric: &str,
        value: String,
        raw: f64,
        bar: &str,
        pass: bool,
    ) {
        self.rows.push(Row {
            experiment,
            subject: subject.into(),
            metric: metric.into(),
            value,
            raw,
            bar: bar.into(),
            verdict: if pass { Verdict::Pass } else { Verdict::Miss },
        });
    }

    /// A correctness property of the candidate. A failure here fails the run.
    fn check(&mut self, ok: bool, what: String) {
        if !ok {
            eprintln!("FAILED: {what}");
            self.failures.push(what);
        }
    }

    fn loaded(&self, kind: Kind) -> &Loaded {
        &self.loaded.iter().find(|(k, _)| *k == kind).unwrap().1
    }

    fn scope(&self, library: u64, plugin: &str) -> Scope {
        Scope::new(&self.catalog, library, plugin, self.state.clone())
    }

    async fn guest(&self, kind: Kind, scope: Scope, opts: Opts) -> Result<Guest> {
        Ok(match kind {
            Kind::Process => Guest::Process(ProcessPlugin::spawn(scope).await?),
            _ => Guest::Wasm(self.rt.instantiate(self.loaded(kind), scope, opts).await?),
        })
    }

    async fn fresh(&self, kind: Kind, library: u64) -> Result<Guest> {
        self.guest(kind, self.scope(library, kind.label()), Opts::default()).await
    }

    // ───────────────────────────── E1 ─────────────────────────────

    async fn calls(&mut self) -> Result<()> {
        const E: &str = "E1 call overhead";
        let payload = "x".repeat(64);

        // The floor: a native call through a function pointer.
        let f: fn(&str) -> String = |s| s.to_owned();
        let mut samples = Vec::with_capacity(100_000);
        for _ in 0..100_000 {
            let t = Instant::now();
            black_box(f(black_box(&payload)));
            samples.push(t.elapsed());
        }
        let s = summarize(samples);
        self.row(E, "native fn", "echo p50 / p99", format!("{} / {}", us(s.p50), us(s.p99)), s.p99.as_secs_f64());

        for kind in Kind::ALL {
            let mut g = self.fresh(kind, BIG).await?;
            let n = if kind == Kind::Process { 20_000 } else { 100_000 };
            for _ in 0..1000 {
                g.noop().await?;
            }
            let mut noop = Vec::with_capacity(n);
            for _ in 0..n {
                let t = Instant::now();
                g.noop().await?;
                noop.push(t.elapsed());
            }
            let mut echo = Vec::with_capacity(n);
            for _ in 0..n {
                let t = Instant::now();
                let back = g.echo(&payload).await?;
                echo.push(t.elapsed());
                debug_assert_eq!(back.len(), 64);
            }
            let (noop, echo) = (summarize(noop), summarize(echo));
            let wasm = kind != Kind::Process;
            let bar = if wasm { "p99 ≤ 10 µs" } else { "" };
            for (metric, s) in [("noop p50 / p99", &noop), ("echo 64 B p50 / p99", &echo)] {
                let value = format!("{} / {}", us(s.p50), us(s.p99));
                if wasm {
                    self.judged(E, kind.label(), metric, value, s.p99.as_secs_f64(), bar, s.p99 <= Duration::from_micros(10));
                } else {
                    self.row(E, kind.label(), metric, value, s.p99.as_secs_f64());
                }
            }
        }
        Ok(())
    }

    // ───────────────────────────── E2 ─────────────────────────────

    async fn instantiate(&mut self) -> Result<()> {
        const E: &str = "E2 warm & footprint";
        let pool = Runtime::new(true)?;
        for kind in Kind::WASM {
            let path = guest_path(kind.file().unwrap());
            let size = std::fs::metadata(&path)?.len();
            self.row(E, kind.label(), "component size", mb(size), size as f64);

            let t = Instant::now();
            let component = wasmtime::component::Component::from_file(&pool.engine, &path)?;
            let compile = t.elapsed();
            self.row(E, kind.label(), "compile (once per version)", ms(compile), compile.as_secs_f64());

            let bytes = component.serialize()?;
            let t = Instant::now();
            // SAFETY: bytes were just produced by `serialize` on this engine.
            let component = unsafe { wasmtime::component::Component::deserialize(&pool.engine, &bytes)? };
            let load = t.elapsed();
            self.row(E, kind.label(), "load precompiled", ms(load), load.as_secs_f64());

            let pooled = pool.prepare(component)?;
            let mut timings = Vec::new();
            for (runtime, name, loaded) in [(&self.rt, "on-demand", self.loaded(kind)), (&pool, "pooling", &pooled)] {
                let mut samples = Vec::new();
                for i in 0..200 {
                    let scope = Scope::new(&self.catalog, BIG, "e2", self.state.clone());
                    let t = Instant::now();
                    let mut inst = runtime.instantiate(loaded, scope, Opts::default()).await?;
                    let took = t.elapsed();
                    if i >= 10 {
                        samples.push(took);
                    }
                    inst.noop().await?;
                }
                timings.push((name, summarize(samples)));
            }
            for (name, s) in timings {
                let metric = format!("instantiate, {name} (p50 / p99)");
                let value = format!("{} / {}", us(s.p50), us(s.p99));
                if kind == Kind::Rust {
                    self.judged(E, kind.label(), &metric, value, s.p50.as_secs_f64(), "< 100 µs", s.p50 < Duration::from_micros(100));
                } else {
                    self.row(E, kind.label(), &metric, value, s.p50.as_secs_f64());
                }
            }
        }

        // Idle footprint: warm instances that have each been called once.
        for kind in Kind::ALL {
            let base = if kind == Kind::Process { 0 } else { stats::self_rss() };
            let mut held = Vec::new();
            let mut at = Vec::new();
            for target in [10usize, 50] {
                while held.len() < target {
                    let mut g = self.fresh(kind, BIG).await?;
                    g.noop().await?;
                    held.push(g);
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
                let rss = match kind {
                    Kind::Process => held
                        .iter()
                        .map(|g| match g {
                            Guest::Process(p) => stats::rss(p.pid()),
                            Guest::Wasm(_) => 0,
                        })
                        .sum(),
                    _ => stats::self_rss().saturating_sub(base),
                };
                at.push((target, rss));
            }
            let per = at[1].1 as f64 / 50.0;
            let value = format!("{} for 10, {} for 50 ({} each)", mb(at[0].1), mb(at[1].1), mb(per as u64));
            self.judged(E, kind.label(), "idle RSS", value, per, "≤ 5 MB per plugin", per <= 5e6);
        }
        Ok(())
    }

    // ───────────────────────────── E3 ─────────────────────────────

    async fn data(&mut self) -> Result<()> {
        const E: &str = "E3 batched data access";
        let count = |scope: &Scope| scope.all().iter().filter(|t| t.title.contains(NEEDLE)).count() as u32;
        let big = self.scope(BIG, "e3");
        let small = self.scope(SMALL, "e3");
        let (want_big, want_small) = (count(&big), count(&small));

        // Floors: borrowed (nothing crosses anything) and paged (the host's own copying).
        let t = Instant::now();
        black_box(count(&big));
        let borrowed = t.elapsed();
        self.row(E, "native, borrowed", "500k tracks", ms(borrowed), borrowed.as_secs_f64());
        let native_paged = |scope: &Scope, batch: u32| {
            let t = Instant::now();
            let (mut found, mut offset) = (0u32, 0u32);
            loop {
                let page = scope.page(offset, batch);
                if page.is_empty() {
                    break;
                }
                found += page.iter().filter(|t| t.title.contains(NEEDLE)).count() as u32;
                offset += page.len() as u32;
            }
            (found, t.elapsed())
        };

        for (batch, library, tracks) in [(1000u32, BIG, BIG_TRACKS), (100, BIG, BIG_TRACKS), (1, SMALL, SMALL_TRACKS)] {
            let scope = if library == BIG { &big } else { &small };
            let want = if library == BIG { want_big } else { want_small };
            let (found, floor) = native_paged(scope, batch);
            assert_eq!(found, want);
            let per = |d: Duration| d.as_secs_f64() * 1e9 / tracks as f64;
            let label = format!("batch {batch}, {}k tracks", tracks / 1000);
            self.row(E, "native, paged", &label, format!("{} ({:.0} ns/track)", ms(floor), per(floor)), floor.as_secs_f64());
            for kind in Kind::ALL {
                let mut g = self.fresh(kind, library).await?;
                g.noop().await?;
                let t = Instant::now();
                let found = g.scan(batch, NEEDLE, LONG).await?;
                let took = t.elapsed();
                self.check(found == want, format!("{}: scan found {found}, expected {want}", kind.label()));
                let ratio = took.as_secs_f64() / floor.as_secs_f64();
                let value = format!("{} ({:.0} ns/track, {ratio:.1}× paged)", ms(took), per(took));
                if batch == 1000 {
                    self.judged(E, kind.label(), &label, value, took.as_secs_f64(), "≤ 2× native paged", ratio <= 2.0);
                } else {
                    self.row(E, kind.label(), &label, value, took.as_secs_f64());
                }
            }
        }
        Ok(())
    }

    // ───────────────────────────── E4 ─────────────────────────────

    async fn hang(&mut self) -> Result<()> {
        const E: &str = "E4 hangs are bounded";
        let budget = Duration::from_millis(50);
        for kind in Kind::ALL {
            let mut g = self.fresh(kind, BIG).await?;
            g.noop().await?;
            let t = Instant::now();
            let result = g.spin(budget).await;
            let took = t.elapsed();
            let bounded = matches!(&result, Err(e) if wasm::is_deadline(e));
            self.check(bounded, format!("{}: spin was not stopped by its deadline: {result:?}", kind.label()));
            let over = took.saturating_sub(budget);
            self.judged(E, kind.label(), "stopped after (50 ms budget)", ms(took), took.as_secs_f64(), "≤ deadline + 5 ms", over <= Duration::from_millis(5));

            // Recovery: a trapped instance cannot be re-entered, so replace it.
            let t = Instant::now();
            let mut g = self.fresh(kind, BIG).await?;
            let ok = g.noop().await.is_ok();
            let took = t.elapsed();
            self.check(ok, format!("{}: no working instance after a hang", kind.label()));
            self.row(E, kind.label(), "back to a working instance", ms(took), took.as_secs_f64());
        }
        Ok(())
    }

    // ───────────────────────────── E5 ─────────────────────────────

    async fn crash(&mut self) -> Result<()> {
        const E: &str = "E5 crashes & memory";
        let limit = 128usize << 20;
        for kind in Kind::ALL {
            let mut g = self.fresh(kind, BIG).await?;
            let r = g.crash().await;
            self.check(r.is_err(), format!("{}: crash returned normally", kind.label()));
            let mut g = self.fresh(kind, BIG).await?;
            let recovered = g.noop().await.is_ok();
            self.check(recovered, format!("{}: no working instance after a crash", kind.label()));
            self.judged(E, kind.label(), "crash", "contained; fresh instance works".into(), 1.0, "contained", r.is_err() && recovered);

            let before = stats::self_rss();
            let opts = Opts { memory_limit: limit, ..Opts::default() };
            let mut g = self.guest(kind, self.scope(BIG, kind.label()), opts).await?;
            let r = g.hog(1024).await;
            let grew = stats::self_rss().saturating_sub(before);
            drop(g);
            tokio::time::sleep(Duration::from_millis(100)).await;
            let kept = stats::self_rss().saturating_sub(before);
            let contained = r.is_err();
            let value = match &r {
                Ok(n) => format!("allocated {n} MiB unchecked"),
                Err(_) => format!("refused at 128 MiB; host grew {}, {} after drop", mb(grew), mb(kept)),
            };
            if kind != Kind::Process {
                self.check(contained, format!("{}: 1 GB allocation was not refused", kind.label()));
            }
            self.judged(E, kind.label(), "allocate 1 GiB (128 MiB limit)", value, grew as f64, "refused", contained);
            let mut g = self.fresh(kind, BIG).await?;
            let recovered = g.noop().await.is_ok();
            self.check(recovered, format!("{}: no working instance after the memory test", kind.label()));
        }
        Ok(())
    }

    // ───────────────────────────── E6 ─────────────────────────────

    async fn concurrency(&mut self) -> Result<()> {
        const E: &str = "E6 no plugin blocks another";
        for kind in [Kind::Rust, Kind::Process] {
            let mut p99s = Vec::new();
            // None, one, and as many spinning plugins as there are worker threads.
            for spinners in [0usize, 1, 4] {
                let mut spinning = Vec::new();
                for _ in 0..spinners {
                    let mut g = self.fresh(kind, BIG).await?;
                    spinning.push(tokio::spawn(async move { g.spin(Duration::from_secs(4)).await }));
                }
                // Requests stamped by a load thread every millisecond, so waiting for a worker
                // held by another plugin counts, and tokio's timer resolution does not.
                let mut senders = Vec::new();
                let mut tasks = Vec::new();
                for _ in 0..7 {
                    let mut g = self.fresh(kind, BIG).await?;
                    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Instant>();
                    senders.push(tx);
                    tasks.push(tokio::spawn(async move {
                        let mut samples = Vec::new();
                        while let Some(sent) = rx.recv().await {
                            g.echo("ping").await.unwrap();
                            samples.push(sent.elapsed());
                        }
                        samples
                    }));
                }
                std::thread::spawn(move || {
                    let end = Instant::now() + Duration::from_secs(2);
                    while Instant::now() < end {
                        for tx in &senders {
                            tx.send(Instant::now()).ok();
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                });
                let mut all = Vec::new();
                for t in tasks {
                    all.extend(t.await?);
                }
                let still_spinning = spinning.iter().all(|t| !t.is_finished());
                for t in &spinning {
                    t.abort();
                }
                self.check(still_spinning, format!("{}: a spinner stopped early", kind.label()));
                let worst = *all.iter().max().unwrap();
                let s = summarize(all);
                let label = match spinners {
                    0 => "all well-behaved".to_owned(),
                    1 => "one plugin spinning".to_owned(),
                    n => format!("{n} spinning (every worker)"),
                };
                let value = format!("p50 {} · p99 {} · max {}", us(s.p50), us(s.p99), us(worst));
                if spinners == 4 && kind != Kind::Process {
                    // Every worker is held; a waiting call gets in at the next tick.
                    let bar = "p99 ≤ 2 ms (a tick + margin)";
                    self.judged(E, kind.label(), &label, value, s.p99.as_secs_f64(), bar, s.p99 <= Duration::from_millis(2));
                } else {
                    self.row(E, kind.label(), &label, value, s.p99.as_secs_f64());
                }
                p99s.push(s.p99);
            }
            let ratio = p99s[1].as_secs_f64() / p99s[0].as_secs_f64();
            self.judged(E, kind.label(), "slowdown from one spinner", format!("{ratio:.1}×"), ratio, "≤ 2×", ratio <= 2.0);
        }
        Ok(())
    }

    // ───────────────────────────── E7 ─────────────────────────────

    async fn yield_(&mut self) -> Result<()> {
        const E: &str = "E7 background work yields";
        let want = self.scope(BIG, "e7").all().iter().filter(|t| t.title.contains(NEEDLE)).count() as u32;
        for kind in Kind::ALL {
            let pause = Arc::new(Pause::default());
            let scope = self.scope(BIG, kind.label());
            let probe = scope.probe.clone();
            let mut g = self.guest(kind, scope, Opts { pause: pause.clone(), ..Opts::default() }).await?;
            let pid = match &g {
                Guest::Process(p) => Some(p.pid()),
                Guest::Wasm(_) => None,
            };
            let task = tokio::spawn(async move { g.scan(100, NEEDLE, LONG).await });

            while probe.calls.load(Ordering::Relaxed) < 20 && !task.is_finished() {
                tokio::task::yield_now().await;
            }
            let stop = |on: bool| match pid {
                // A process can only be paused from outside, by signal.
                Some(pid) => unsafe {
                    libc::kill(pid as i32, if on { libc::SIGSTOP } else { libc::SIGCONT });
                },
                None => pause.set(on),
            };
            let paused_at = library::now_ns();
            stop(true);
            tokio::time::sleep(Duration::from_millis(20)).await;
            let settled = probe.calls.load(Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(50)).await;
            let still = probe.calls.load(Ordering::Relaxed) == settled;
            let parked_after = Duration::from_nanos(probe.last_ns().saturating_sub(paused_at));
            stop(false);
            let found = task.await??;

            let finished_early = !still && settled == 0;
            self.check(!finished_early, format!("{}: scan finished before it could be paused", kind.label()));
            self.check(still, format!("{}: kept reading data while paused", kind.label()));
            self.check(found == want, format!("{}: resumed scan found {found}, expected {want}", kind.label()));
            let (how, value) = match pid {
                Some(_) => (" (SIGSTOP)", "immediate".to_owned()),
                // Zero: the pause landed mid-page, and the plugin never asked for another.
                None if parked_after.is_zero() => ("", "before its next read".to_owned()),
                None => ("", us(parked_after)),
            };
            self.judged(E, kind.label(), &format!("parked after pause{how}"), value, parked_after.as_secs_f64(), "≤ 2 ms", still && parked_after <= Duration::from_millis(2));
        }
        Ok(())
    }

    // ───────────────────────────── E8 ─────────────────────────────

    async fn state(&mut self) -> Result<()> {
        const E: &str = "E8 state & scoping";
        for kind in Kind::ALL {
            let mut g = self.fresh(kind, BIG).await?;
            let warm = [g.counter().await?, g.counter().await?, g.counter().await?];
            self.check(warm == [1, 2, 3], format!("{}: in-instance counter gave {warm:?}", kind.label()));

            let name = format!("e8 {}", kind.label());
            let mut g = self.guest(kind, self.scope(BIG, &name), Opts::default()).await?;
            let first = [g.persisted().await?, g.persisted().await?];
            drop(g);
            let mut g = self.guest(kind, self.scope(BIG, &name), Opts::default()).await?;
            let after = g.persisted().await?;
            let persisted = first == [1, 2] && after == 3;
            self.check(persisted, format!("{}: persisted counter gave {first:?} then {after}", kind.label()));

            let mut small = self.fresh(kind, SMALL).await?;
            let mut big = self.fresh(kind, BIG).await?;
            let (s, b) = (small.scan(1000, "", LONG).await?, big.scan(1000, "", LONG).await?);
            let scoped = s as usize == SMALL_TRACKS && b as usize == BIG_TRACKS;
            self.check(scoped, format!("{}: scoped scans saw {s} and {b} tracks", kind.label()));

            let value = format!("warm {warm:?}; persisted 1, 2 → reinstantiate → {after}; libraries see {s} / {b}");
            self.judged(E, kind.label(), "state and scope", value, 1.0, "exact", warm == [1, 2, 3] && persisted && scoped);
        }
        Ok(())
    }

    // ───────────────────────────── Output ─────────────────────────────

    fn print(&self) {
        let machine = machine();
        println!("\n## Plugin spike results\n\n{machine}\n");
        let mut last = "";
        for r in &self.rows {
            if r.experiment != last {
                println!("\n### {}\n\n| Subject | Measure | Result | Bar | |\n|---|---|---|---|---|", r.experiment);
                last = r.experiment;
            }
            let verdict = match r.verdict {
                Verdict::Pass => "✅",
                Verdict::Miss => "❌",
                Verdict::Info => "",
            };
            println!("| {} | {} | {} | {} | {verdict} |", r.subject, r.metric, r.value, r.bar);
        }
        if self.failures.is_empty() {
            println!("\nAll correctness checks passed.");
        } else {
            println!("\n{} correctness check(s) failed:", self.failures.len());
            for f in &self.failures {
                println!("- {f}");
            }
        }
    }
}

fn machine() -> String {
    let sh = |cmd: &str| {
        std::process::Command::new("sh")
            .args(["-c", cmd])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default()
    };
    let cpu = sh("sysctl -n machdep.cpu.brand_string 2>/dev/null || grep -m1 'model name' /proc/cpuinfo | cut -d: -f2");
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    format!(
        "Machine: {cpu}, {cores} cores, {} {}. Host runtime: tokio, 4 worker threads (Pi 4-like). Epoch tick: {} µs.",
        std::env::consts::OS,
        std::env::consts::ARCH,
        wasm::tick().as_micros()
    )
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut which = "all".to_owned();
    let mut json: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--json" => json = args.next().map(PathBuf::from),
            other => which = other.to_owned(),
        }
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?;
    runtime.block_on(async move {
        eprintln!("generating catalog ({BIG_TRACKS} + {SMALL_TRACKS} tracks)…");
        let catalog = Catalog::generate(BIG_TRACKS, SMALL_TRACKS);
        let rt = Runtime::new(false)?;
        let mut loaded = Vec::new();
        for kind in Kind::WASM {
            eprintln!("compiling {}…", kind.label());
            loaded.push((kind, rt.load(&guest_path(kind.file().unwrap()))?));
        }
        let mut h = Harness { rt, loaded, catalog, state: Arc::default(), rows: Vec::new(), failures: Vec::new() };
        let all = which == "all";
        macro_rules! run {
            ($name:literal, $f:ident) => {
                if all || which == $name {
                    eprintln!("running {}…", $name);
                    h.$f().await?;
                }
            };
        }
        run!("calls", calls);
        run!("instantiate", instantiate);
        run!("data", data);
        run!("hang", hang);
        run!("crash", crash);
        run!("concurrency", concurrency);
        run!("yield", yield_);
        run!("state", state);
        if h.rows.is_empty() {
            bail!("unknown experiment {which:?}");
        }
        h.print();
        if let Some(path) = json {
            let doc = serde_json::json!({ "machine": machine(), "rows": h.rows, "failures": h.failures });
            std::fs::write(&path, serde_json::to_vec_pretty(&doc)?)?;
            eprintln!("wrote {}", path.display());
        }
        if !h.failures.is_empty() {
            std::process::exit(1);
        }
        Ok(())
    })
}
