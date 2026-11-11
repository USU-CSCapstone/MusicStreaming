//! What the four query costs, tag walks, counted totals, leading pages and big-tag recipes,
//! cost on the generated library
//! (`design/database.md` §4), read from the device file `write_device_scale_files` writes:
//! a tag-filtered page walking the index (with a tag whose rows all sort last, which a walk reads
//! through), a counted total asked again, a page that leads from a tag or an artist, and a
//! recipe resolved from a big tag. Run it with
//! `cargo test --release -p jewelcase-catalog --lib page::bench -- --ignored --nocapture`.
//! It copies the file into the temporary folder to add the correlated tag, and prints each
//! case's median and slowest time over `RUNS` reads; it fails only on a wrong answer.

use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde_json::{Value, json};

use super::Totals;
use crate::query;

/// How many times each case is read.
const RUNS: usize = 30;

/// The JSON text answering the query `name` with `params` for user 1 in `library`, with totals
/// remembered in `totals`.
fn ask_text(
    conn: &Connection,
    totals: &Totals,
    library: i64,
    name: &str,
    params: &Value,
) -> String {
    query::answer(conn, 1, library, name, &params.to_string(), None, Some(totals)).unwrap()
}

/// The answer of `ask_text`, parsed.
fn ask(conn: &Connection, totals: &Totals, library: i64, name: &str, params: &Value) -> Value {
    serde_json::from_str(&ask_text(conn, totals, library, name, params)).unwrap()
}

/// Runs `read` `RUNS` times and prints the median and the slowest, in milliseconds.
fn time(what: &str, mut read: impl FnMut() -> usize) {
    let mut times: Vec<Duration> = Vec::new();
    let mut rows = 0;
    for _ in 0..RUNS {
        let started = Instant::now();
        rows = read();
        times.push(started.elapsed());
    }
    let first = times[0];
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "{what:<58} rows {rows:>6}  first {:>8.2} ms  median {:>8.2} ms  max {:>8.2} ms",
        ms(first),
        ms(times[RUNS / 2]),
        ms(times[RUNS - 1])
    );
}

/// The ID, as a decimal string, of the tag called `name`.
fn tag_named(conn: &Connection, name: &str) -> String {
    conn.query_row("SELECT id FROM tags WHERE name = ?1", [name], |row| row.get::<_, i64>(0))
        .unwrap()
        .to_string()
}

#[test]
#[ignore = "benchmark"]
fn benchmark_query_costs() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/catalog-scale/device.db");
    let copy = std::env::temp_dir().join("jewelcase-bench-query-costs.db");
    std::fs::copy(&source, &copy).unwrap();
    let conn = Connection::open(&copy).unwrap();
    let library: i64 = conn.query_row("SELECT id FROM libraries", [], |row| row.get(0)).unwrap();
    // One track in a hundred, all of which sort last by name: a walk meets them after the rest.
    let late = 5_000;
    conn.execute_batch(&format!(
        "INSERT INTO tags (id, library_id, type, name, sort_key, track_count) \
         VALUES (1, {library}, 'genre', 'Late', CAST('late' AS BLOB), {late}); \
         INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) \
         SELECT {library}, id, 1, 'Late' FROM tracks WHERE library_id = {library} \
         ORDER BY sort_key DESC LIMIT {late}; \
         ANALYZE"
    ))
    .unwrap();
    let totals = Totals::default();
    let tag = |name: &str| tag_named(&conn, name);
    let (common, mid, small) = (tag("Genre 0"), tag("Genre 9"), tag("Genre 39"));
    let (mood, late) = (tag("Mood 100000ppm"), "1".to_owned());
    let page = |extra: Value, limit: u32| {
        let mut params = json!({ "limit": limit });
        params.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        params
    };
    let read = |params: &Value| {
        let page = ask(&conn, &totals, library, "listTracks", params);
        page["items"].as_array().unwrap().len()
    };
    let deep = |params: &Value| {
        let first = ask(&conn, &totals, library, "listTracks", params);
        let mut params = params.clone();
        params["cursor"] = first["nextCursor"].clone();
        params
    };

    println!("\nwalks (tag share: common 29%, mid 2%, late 1% sorted last)");
    for (what, tags) in [("common", &common), ("mid", &mid), ("late", &late)] {
        let first = page(json!({ "tagAny": tags }), 100);
        time(&format!("first page, {what}"), || read(&first));
        let second = deep(&first);
        time(&format!("deep page, {what}"), || read(&second));
    }

    println!("\ncounted totals (genre common and mood, so the total is counted)");
    let counted = page(json!({ "tagAny": common, "tagAll": mood }), 100);
    time("first page, counted total", || read(&counted));
    let second = deep(&counted);
    time("deep page, counted total", || read(&second));

    println!("\nleading pages (a small tag, the artist with the most tracks)");
    let small = page(json!({ "tagAny": small }), 100);
    time("first page, small tag", || read(&small));
    let artist: i64 = conn
        .query_row(
            "SELECT artist_id FROM track_artists GROUP BY artist_id ORDER BY count(*) DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let artist = page(json!({ "artistId": artist.to_string() }), 100);
    time("first page, biggest artist", || read(&artist));
    let second = deep(&artist);
    time("deep page, biggest artist", || read(&second));

    println!("\nrecipes (the library by name under a tag)");
    for (what, tag) in [("common (leads at no share)", &common), ("mood", &mood), ("mid", &mid)] {
        let recipe = json!({ "recipe": { "type": "library", "sort": "name", "order": "asc",
            "filters": { "tagAny": [tag] } } });
        time(&format!("resolve, {what}"), || {
            ask(&conn, &totals, library, "resolveRecipe", &recipe)["ids"].as_array().unwrap().len()
        });
    }
    let _ = std::fs::remove_file(&copy);
}

/// What an answer costs to serialize: a page of 500 tracks, and a recipe of about 10,000 IDs,
/// each asked for as the device asks, JSON out. Run with `cargo test --release -p
/// jewelcase-catalog --lib page::bench::benchmark_answer -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn benchmark_answer() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/catalog-scale/device.db");
    let conn = Connection::open(source).unwrap();
    let library: i64 = conn.query_row("SELECT id FROM libraries", [], |row| row.get(0)).unwrap();
    let mid = tag_named(&conn, "Genre 9");
    let totals = Totals::default();
    let page = json!({ "limit": 500 });
    let recipe = json!({ "recipe": { "type": "library", "sort": "name", "order": "asc",
        "filters": { "tagAny": [mid] } } });
    for (what, name, params) in [
        ("listTracks limit 500", "listTracks", &page),
        ("resolveRecipe, one tag", "resolveRecipe", &recipe),
    ] {
        // The text is timed alone; its rows are counted once, outside the timing.
        let parsed = ask(&conn, &totals, library, name, params);
        let rows = parsed["items"].as_array().or(parsed["ids"].as_array()).unwrap().len();
        let mut bytes = 0;
        time(&format!("{what}, answered"), || {
            bytes = ask_text(&conn, &totals, library, name, params).len();
            rows
        });
        println!("    {bytes} bytes");
    }
}
