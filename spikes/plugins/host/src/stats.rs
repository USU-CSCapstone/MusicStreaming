//! Percentiles and memory sampling.

use std::time::Duration;

pub struct Summary {
    pub p50: Duration,
    pub p99: Duration,
}

pub fn summarize(mut samples: Vec<Duration>) -> Summary {
    samples.sort_unstable();
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    Summary { p50: at(0.50), p99: at(0.99) }
}

/// Resident set size of a process in bytes, as `ps` reports it.
pub fn rss(pid: u32) -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().unwrap_or(0) * 1024
}

pub fn self_rss() -> u64 {
    rss(std::process::id())
}

pub fn us(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us < 10.0 { format!("{us:.2} µs") } else if us < 1000.0 { format!("{us:.0} µs") } else { ms(d) }
}

pub fn ms(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1e3;
    if ms < 10.0 { format!("{ms:.2} ms") } else { format!("{ms:.0} ms") }
}

pub fn mb(bytes: u64) -> String {
    if bytes < 1_000_000 { format!("{:.0} KB", bytes as f64 / 1e3) } else { format!("{:.1} MB", bytes as f64 / 1e6) }
}
