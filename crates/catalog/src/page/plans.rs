//! A device's lists read their pages from the sort indexes of `schema.sql`, which hold no
//! trailing `id`: the rowid orders ties, serves a descending walk and a cursor's seek. Pages that
//! lead, walk within a budget or resolve from a big tag read the same rows as a plain walk.

use rusqlite::Connection;

use super::keyset::{Part, part_query, read_page};
use super::{Order, Total, assert_indexed};
use crate::albums::{self, AlbumSort};
use crate::artists::{self, ArtistSort};
use crate::credit::ArtistCredit;
use crate::playlists::{self, PlaylistSort};
use crate::tag_filter::{LEAD_SHARE, TagFilter};
use crate::tags::{self, TagSort};
use crate::tracks::{self, TrackSort, WITHIN_ALBUM};
use crate::{AnyId, create_schema};

/// A device database with a library of 20,000 tracks, albums, artists, tags and playlists: some
/// unknown, some unnumbered or undated, keys that do not follow ids, half the entities carrying
/// tag 1 (too many to lead, as its counts say) and the rest one of 299 others, and `ANALYZE` run
/// as a snapshot's is.
pub(super) fn device() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    create_schema(&conn).unwrap();
    let key =
        |prefix: &str| format!("CAST('{prefix}' || printf('%06d', (i * 7919) % 100003) AS BLOB)");
    let unknown = |prefix: &str, every: i64| {
        format!("CASE WHEN i % {every} = 0 THEN x'' ELSE {} END", key(prefix))
    };
    conn.execute_batch(&format!(
        "WITH RECURSIVE n (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000) \
         INSERT INTO artists (id, library_id, name, sort_key, added_at) \
         SELECT i, 1, 'Artist ' || i, {artist_key}, i FROM n; \
         WITH RECURSIVE n (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000) \
         INSERT INTO albums (id, library_id, title, sort_key, artist_sort_key, release_date, \
             duration_us, added_at) \
         SELECT i, 1, 'Album ' || i, {album_key}, {artist_key}, \
             CASE WHEN i % 6 = 0 THEN NULL ELSE printf('%04d', 1950 + i % 75) END, \
             (i * 7919) % 600000000, i FROM n; \
         WITH RECURSIVE n (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000) \
         INSERT INTO tracks (id, library_id, album_id, file_size, title, sort_key, \
             artist_sort_key, album_sort_key, disc_number, track_number, release_date, codec, \
             container, lossless, sample_rate_hz, channels, duration_us, added_at) \
         SELECT i, 1, 1 + i % 2000, 1, 'Track ' || i, {track_key}, \
             CASE WHEN i % 997 = 0 THEN x'' ELSE CAST(printf('artist %05d', i % 3000) AS BLOB) END, \
             {album_key}, 1 + i % 2, CASE WHEN i % 13 = 0 THEN NULL ELSE i % 12 END, \
             CASE WHEN i % 6 = 0 THEN NULL ELSE printf('%04d', 1950 + i % 75) END, \
             'flac', 'flac', 1, 44100, 2, (i * 7919) % 600000000, i FROM n; \
         WITH RECURSIVE n (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 300) \
         INSERT INTO tags (id, library_id, type, name, sort_key, track_count, album_count, \
             artist_count) \
         SELECT i, 1, 'genre', 'Genre ' || i, CAST('genre ' || i AS BLOB), \
             CASE i WHEN 1 THEN 10000 ELSE 34 END, CASE i WHEN 1 THEN 10000 ELSE 34 END, \
             CASE i WHEN 1 THEN 10000 ELSE 34 END FROM n; \
         INSERT INTO libraries (id, track_count, album_count, artist_count) \
         VALUES (1, 20000, 20000, 20000); \
         WITH RECURSIVE n (i) AS (SELECT 301 UNION ALL SELECT i + 1 FROM n WHERE i < 20300) \
         INSERT INTO tags (id, library_id, type, name, sort_key, track_count) \
         SELECT i, 1, CASE WHEN i % 2 = 0 THEN 'genre' ELSE 'mood' END, 'Tag ' || i, {tag_key}, \
             i % 50 FROM n; \
         INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) \
         SELECT 1, id, CASE WHEN id % 2 = 1 THEN 1 ELSE 2 + id % 299 END, 'Genre' FROM tracks; \
         INSERT INTO album_tags (library_id, album_id, tag_id, track_count) \
         SELECT 1, id, CASE WHEN id % 2 = 1 THEN 1 ELSE 2 + id % 299 END, 1 FROM albums; \
         INSERT INTO artist_tags (library_id, artist_id, tag_id, track_count) \
         SELECT 1, id, CASE WHEN id % 2 = 1 THEN 1 ELSE 2 + id % 299 END, 1 FROM artists; \
         WITH RECURSIVE n (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000) \
         INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, pinned, \
             created_at, modified_at) \
         SELECT i, 1, 1, 'Playlist ' || i, 'playlist ' || i, {playlist_key}, i % 7 = 0, \
             i * 3 % 20011, i * 5 % 20011 FROM n; \
         ANALYZE",
        artist_key = unknown("artist ", 1000),
        album_key = unknown("album ", 200),
        track_key = key("track "),
        tag_key = key("tag "),
        playlist_key = key("playlist "),
    ))
    .unwrap();
    conn
}

/// Checks that a tag-filtered list keeps its stored total, which only a walk of a sort's index
/// does: a list led from its tag counts its rows instead, and such leads are exempt.
fn walks(source: &super::Source, filter: &TagFilter) {
    if !filter.any.is_empty() {
        assert!(matches!(source.total, Total::Stored(10_000)));
    }
}

#[test]
fn every_list_reads_each_sort_and_walk_from_one_index_range() {
    let conn = device();
    let tagged = TagFilter { any: vec![AnyId(Some(1))], ..TagFilter::default() };
    let none = TagFilter::default();
    for sort in [
        TrackSort::Name,
        TrackSort::Artist,
        TrackSort::Album,
        TrackSort::DateAdded,
        TrackSort::ReleaseDate,
        TrackSort::Duration,
    ] {
        for filter in [&none, &tagged] {
            let source =
                tracks::source(&conn, 1, None, None, ArtistCredit::Any, filter, LEAD_SHARE)
                    .unwrap();
            walks(&source, filter);
            assert_indexed(&conn, &source, sort.sort()).unwrap();
        }
    }
    let one_album =
        tracks::source(&conn, 1, Some(AnyId(Some(7))), None, ArtistCredit::Any, &none, LEAD_SHARE)
            .unwrap();
    assert_indexed(&conn, &one_album, &WITHIN_ALBUM).unwrap();
    for sort in [
        AlbumSort::Name,
        AlbumSort::Artist,
        AlbumSort::DateAdded,
        AlbumSort::ReleaseDate,
        AlbumSort::Duration,
    ] {
        for filter in [&none, &tagged] {
            let source = albums::source(&conn, 1, None, ArtistCredit::Any, filter).unwrap();
            walks(&source, filter);
            assert_indexed(&conn, &source, sort.sort()).unwrap();
        }
    }
    for sort in [ArtistSort::Name, ArtistSort::DateAdded] {
        for filter in [&none, &tagged] {
            let source = artists::source(&conn, 1, filter).unwrap();
            walks(&source, filter);
            assert_indexed(&conn, &source, sort.sort()).unwrap();
        }
    }
    for sort in [TagSort::Name, TagSort::TrackCount] {
        for q in [None, Some("a")] {
            let source = tags::source(1, Some("genre".to_owned()), q);
            assert_indexed(&conn, &source, sort.sort()).unwrap();
        }
    }
    for sort in [PlaylistSort::Name, PlaylistSort::Created, PlaylistSort::Modified] {
        for q in [None, Some("playlist 1")] {
            assert_indexed(&conn, &playlists::source(1, 1, q), sort.sort()).unwrap();
        }
    }
}

/// The steps of the plan of `sql`, each as (id, parent, what it does).
fn plan(
    conn: &Connection,
    sql: &str,
    params: Vec<rusqlite::types::Value>,
) -> Vec<(i64, i64, String)> {
    conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap()
        .query_map(rusqlite::params_from_iter(params), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn a_page_that_leads_sorts_ids_and_keys_and_joins_the_columns_after() {
    let conn = device();
    // Tag 3 holds 34 of 20,000 tracks, few enough to start from.
    let tagged = TagFilter { any: vec![AnyId(Some(3))], ..TagFilter::default() };
    let source =
        tracks::source(&conn, 1, None, None, ArtistCredit::Any, &tagged, LEAD_SHARE).unwrap();
    assert!(source.back.is_some() && source.walk.is_none());
    for sort in [TrackSort::Name, TrackSort::Artist, TrackSort::Album, TrackSort::ReleaseDate] {
        for order in [Order::Asc, Order::Desc] {
            for part in [Part::Known, Part::Unknown] {
                let (sql, params) = part_query(&source, sort.sort(), order, None, part, 101, None);
                let steps = plan(&conn, &sql, params);
                // The sort is inside the subquery that picks the page's rows, which reads only
                // the lead's links and the tracks' keys.
                let sorted =
                    steps.iter().find(|(_, _, step)| step.contains("TEMP B-TREE FOR ORDER BY"));
                let (_, page, _) = sorted.expect("the lead's rows are sorted");
                let inside = |id: i64| {
                    let mut at = id;
                    while let Some((_, parent, _)) = steps.iter().find(|(step, _, _)| *step == at) {
                        if parent == page {
                            return true;
                        }
                        at = *parent;
                    }
                    false
                };
                let (within, after): (Vec<_>, Vec<_>) =
                    steps.iter().partition(|(id, _, _)| inside(*id) || id == page);
                let within: Vec<&str> = within.iter().map(|(_, _, step)| step.as_str()).collect();
                let after: Vec<&str> = after.iter().map(|(_, _, step)| step.as_str()).collect();
                let label = format!("{} {order:?}: {steps:?}", sort.name());
                assert!(within.iter().any(|step| step.contains("track_tags")), "{label}");
                assert!(!within.iter().any(|step| step.contains("CORRELATED")), "{label}");
                assert!(
                    after.iter().any(|step| step.contains("SEARCH t USING INTEGER")),
                    "{label}"
                );
                assert!(after.iter().any(|step| step.contains("CORRELATED")), "{label}");
                assert!(
                    !after.iter().any(|step| step.contains("TEMP B-TREE FOR ORDER BY")),
                    "{label}"
                );
            }
        }
    }
}

/// A device with 20,000 tracks and, beside `device`'s tags, three moods. Tag 30000 is on the 2,000
/// tracks that sort last by name, which a walk meets after all the rest. Tag 30001 is on 2,000
/// from the 1,501st, which a page fills from after a window or two of reading. Tag 30002 is on 400
/// from the 2,200th from the end, where a page of ten read by name descending ends its first
/// window (11 rows × 50 rows each × 4).
fn correlated() -> Connection {
    let conn = device();
    conn.execute_batch(
        "INSERT INTO tags (id, library_id, type, name, sort_key, track_count) \
         VALUES (30000, 1, 'mood', 'Late', CAST('late' AS BLOB), 2000), \
                (30001, 1, 'mood', 'Later', CAST('later' AS BLOB), 2000), \
                (30002, 1, 'mood', 'Edge', CAST('edge' AS BLOB), 400); \
         INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) \
         SELECT 1, id, 30000, 'Late' FROM tracks ORDER BY sort_key DESC LIMIT 2000; \
         INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) \
         SELECT 1, id, 30001, 'Later' FROM tracks ORDER BY sort_key LIMIT 2000 OFFSET 1500; \
         INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) \
         SELECT 1, id, 30002, 'Edge' FROM tracks ORDER BY sort_key DESC LIMIT 400 OFFSET 2199; \
         ANALYZE",
    )
    .unwrap();
    conn
}

/// What a read of the tracks carrying one tag, a page after another, gives.
struct Pages {
    ids: Vec<i64>,
    totals: Vec<i64>,
    /// Whether the first page's walk ran over its budget.
    led: bool,
}

/// The filter for tracks carrying tag `tag`.
fn any(tag: i64) -> TagFilter {
    TagFilter { any: vec![AnyId(Some(tag))], ..TagFilter::default() }
}

/// Every page of `limit` tracks carrying the tags `tagged` names in `sort` and `order`, after
/// leaving out `hidden`, read from the source as it is made, or as a plain walk of the index if
/// `walk` is false.
fn pages(
    conn: &Connection,
    tagged: &TagFilter,
    hidden: &[i64],
    (sort, order): (TrackSort, Order),
    limit: u32,
    walk: bool,
) -> Pages {
    let mut source =
        tracks::source(conn, 1, None, None, ArtistCredit::Any, tagged, LEAD_SHARE).unwrap();
    crate::hidden::leave_out_ids(conn, &mut source, "t.id", hidden).unwrap();
    if !walk {
        source.walk = None;
    }
    let map = |row: &rusqlite::Row| row.get::<_, i64>(0);
    let mut cursor = None;
    let mut read = Pages { ids: Vec::new(), totals: Vec::new(), led: false };
    loop {
        let request = super::Request::new(sort.sort(), order, cursor, Some(limit)).unwrap();
        if read.totals.is_empty() && walk {
            let walked = read_page(conn, &source, source.walk.as_deref(), &request, &mut { map });
            read.led = walked.unwrap().is_none();
        }
        let page = request.read(conn, &source, map).unwrap();
        read.ids.extend(page.items);
        read.totals.push(page.total);
        cursor = page.next_cursor;
        if cursor.is_none() {
            return read;
        }
    }
}

#[test]
fn a_walk_whose_tags_sort_last_leads_instead_and_reads_the_same_pages() {
    let conn = correlated();
    // Ten rows a page, read as eleven: the budget is 11 × 256 = 2,816 rows, a sixth of the 18,000
    // the walk reads before the tag's first.
    // The tracks left out are the first of the tag's, so the first page, which leads, shows it.
    let hidden: Vec<i64> = conn
        .prepare(
            "SELECT t.id FROM tracks t JOIN track_tags tt ON tt.track_id = t.id \
             WHERE tt.tag_id = 30000 ORDER BY t.sort_key LIMIT 7",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let name = (TrackSort::Name, Order::Asc);
    for hidden in [&[][..], &hidden] {
        let led = pages(&conn, &any(30000), hidden, name, 10, true);
        assert!(led.led, "the walk should have run over its budget");
        let walked = pages(&conn, &any(30000), hidden, name, 10, false);
        assert_eq!((&led.ids, &led.totals), (&walked.ids, &walked.totals));
        let left = 2000 - i64::try_from(hidden.len()).unwrap();
        assert_eq!(led.ids.len(), usize::try_from(left).unwrap());
        assert!(led.totals.iter().all(|&total| total == left));
        assert!(hidden.iter().all(|id| !led.ids.contains(id)));
    }
}

#[test]
fn a_walk_that_fills_its_page_within_its_budget_does_not_lead_and_reads_the_same_pages() {
    let conn = correlated();
    // Half the library carries tag 1, spread through it; tag 30001 starts 1,500 rows in, past the
    // first window and inside the budget.
    for (tag, limit) in [(1, 10), (30001, 10), (30001, 100)] {
        let name = (TrackSort::Name, Order::Asc);
        let read = pages(&conn, &any(tag), &[], name, limit, true);
        assert!(!read.led, "tag {tag} should fill within its budget");
        let walked = pages(&conn, &any(tag), &[], name, limit, false);
        assert_eq!((&read.ids, &read.totals), (&walked.ids, &walked.totals));
    }
}

#[test]
fn leading_and_walking_read_every_sort_and_order_alike() {
    let conn = correlated();
    let hidden = [1, 2, 3, 19_998, 19_999, 20_000];
    for sort in [TrackSort::Name, TrackSort::Artist, TrackSort::ReleaseDate, TrackSort::Duration] {
        for order in [Order::Asc, Order::Desc] {
            for tag in [30000, 30001] {
                let kind = (sort, order);
                let read = pages(&conn, &any(tag), &hidden, kind, 10, true);
                let walked = pages(&conn, &any(tag), &hidden, kind, 10, false);
                let label = format!("{} {order:?} tag {tag}", sort.name());
                assert_eq!(read.ids, walked.ids, "{label}");
                assert_eq!(read.totals, walked.totals, "{label}");
            }
        }
    }
}

#[test]
fn a_big_tags_recipe_walks_the_index_for_ids_and_filters_them_in_a_set() {
    let conn = device();
    let big = TagFilter { any: vec![AnyId(Some(1))], ..TagFilter::default() };
    let (source, tagged) =
        tracks::resolving(&conn, 1, 1, None, None, ArtistCredit::Any, &big).unwrap();
    let tagged = tagged.expect("half the library is too many to start from");
    assert_eq!(tagged.len(), 10_000);
    for sort in [TrackSort::Name, TrackSort::DateAdded] {
        let (sql, params) =
            part_query(&source, sort.sort(), Order::Asc, None, Part::Known, u32::MAX, None);
        assert!(!sql.contains("EXISTS"), "{sql}");
        let steps = plan(&conn, &sql, params);
        let steps: Vec<&str> = steps.iter().map(|(_, _, step)| step.as_str()).collect();
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert!(steps[0].contains("COVERING INDEX tracks_by_"), "{steps:?}");
    }
    // The ids are those a walk checking each row would give.
    let order = tracks::TrackOrder { sort: TrackSort::Name, order: Order::Asc };
    let ids = tracks::resolve(&conn, 1, 1, None, None, ArtistCredit::Any, &big, order).unwrap();
    let mut checked =
        tracks::source(&conn, 1, None, None, ArtistCredit::Any, &big, LEAD_SHARE).unwrap();
    checked.walk = None;
    checked.select = &["t.id"];
    checked.joins = "";
    let expected = super::ids(&conn, &checked, TrackSort::Name.sort(), Order::Asc).unwrap();
    assert_eq!(ids, expected);
    assert_eq!(ids.len(), 10_000);

    // A smaller tag leads, and no set is kept.
    let small = TagFilter { any: vec![AnyId(Some(3))], ..TagFilter::default() };
    let (_, tagged) =
        tracks::resolving(&conn, 1, 1, None, None, ArtistCredit::Any, &small).unwrap();
    assert!(tagged.is_none());
}

#[test]
fn a_walk_over_two_groups_leads_with_the_other_groups_checked() {
    let conn = correlated();
    // Tag 30000 is a mood and tag 1 a genre: two groups, and only the 1,000 or so tracks of the
    // first that carry the second match. The smaller group sorts last, so the page leads.
    let both = TagFilter { all: vec![AnyId(Some(30000)), AnyId(Some(1))], ..TagFilter::default() };
    let name = (TrackSort::Name, Order::Asc);
    let led = pages(&conn, &both, &[], name, 10, true);
    assert!(led.led, "the walk should have run over its budget");
    let walked = pages(&conn, &both, &[], name, 10, false);
    assert_eq!((&led.ids, &led.totals), (&walked.ids, &walked.totals));
    assert!((900..1100).contains(&led.ids.len()), "{}", led.ids.len());
}

#[test]
fn a_descending_walk_loses_no_row_at_the_edge_of_a_window() {
    let conn = correlated();
    let desc = (TrackSort::Name, Order::Desc);
    let read = pages(&conn, &any(30002), &[], desc, 10, true);
    assert!(!read.led);
    assert_eq!(read.ids.len(), 400);
    let walked = pages(&conn, &any(30002), &[], desc, 10, false);
    assert_eq!(read.ids, walked.ids);
}
