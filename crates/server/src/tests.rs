use std::fs;
use std::time::Duration;

use futures_util::StreamExt;
use jewelcase_plugins::Permission::Schedule;
use rusqlite::OptionalExtension;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{Instant, sleep, timeout};
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::api::testing::player::{report, start_playing};
use crate::api::testing::socket::connect_with;
use crate::api::testing::{TOKEN, add_device};
use crate::db::testing;
use crate::plugins::testing::{grant, install, manifest};
use crate::serve::DRAIN_TIMEOUT;

/// How long `run` gets to refuse: a server that does not stop has started serving.
const REFUSED: Duration = Duration::from_secs(10);

/// A server configured with `data_dir` and `music`, on any free port.
fn config(data_dir: &Path, music: Option<&Path>) -> Config {
    let music = music.map(Path::to_owned);
    Config { data_dir: data_dir.to_owned(), music, port: 0, ..Config::default() }
}

#[tokio::test]
async fn a_data_directory_inside_the_music_stops_the_server_having_written_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let music = temp.path().canonicalize().unwrap();
    let refused = timeout(REFUSED, run(config(&music.join("data"), Some(&music)))).await;
    let error = refused.unwrap().unwrap_err();
    assert!(format!("{error:#}").contains("overlap"), "{error:#}");
    assert_eq!(fs::read_dir(music.join("data")).unwrap().count(), 0);
}

#[tokio::test]
async fn a_library_root_holding_the_data_directory_stops_the_server() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    {
        let data_dir = DataDir::open(&data, None).unwrap();
        let db = Database::open(&data_dir.state().join("jewelcase.db")).unwrap();
        let music = temp.path().canonicalize().unwrap();
        db.write(move |tx| testing::library(tx, 1, "Music", &music)).await.unwrap();
    }
    // Music not mounted yet is passed over, so the server still gets to the stored roots.
    let unmounted = temp.path().join("unmounted");
    for music in [None, Some(unmounted.as_path())] {
        let error = timeout(REFUSED, run(config(&data, music))).await.unwrap().unwrap_err();
        assert!(format!("{error:#}").contains("overlap"), "{music:?}: {error:#}");
    }
}

#[test]
fn music_not_there_yet_is_not_checked() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(resolve_music(temp.path()), Some(temp.path().canonicalize().unwrap()));
    assert_eq!(resolve_music(&temp.path().join("unmounted")), None);
}

#[tokio::test]
async fn the_music_setting_creates_the_first_library_and_never_a_second() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = DataDir::open(&temp.path().join("data"), None).unwrap();
    let db = Database::open(&data_dir.state().join("jewelcase.db")).unwrap();
    let (music, other) = (temp.path().join("music"), temp.path().join("other"));
    fs::create_dir(&music).unwrap();
    fs::create_dir(&other).unwrap();
    create_first_library(&db, &music).await.unwrap();
    create_first_library(&db, &music).await.unwrap();
    create_first_library(&db, &other).await.unwrap();
    assert_eq!(db.read(libraries::all).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_first_start_needs_the_music_there() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = DataDir::open(&temp.path().join("data"), None).unwrap();
    let db = Database::open(&data_dir.state().join("jewelcase.db")).unwrap();
    let error = create_first_library(&db, &temp.path().join("unmounted")).await.unwrap_err();
    assert!(format!("{error:#}").contains(config::MUSIC), "{error:#}");
}

/// A copy of an image no library holds is deleted before the server takes requests.
#[tokio::test]
async fn the_image_cache_is_swept_as_the_server_starts() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let cache = DataDir::open(&data, None).unwrap().cache().to_owned();
    let unheld = cache.join("images/1/01/01-64.jpg");
    fs::create_dir_all(unheld.parent().unwrap()).unwrap();
    fs::write(&unheld, "").unwrap();
    let ran = timeout(REFUSED, run_until(config(&data, None), std::future::ready(())));
    ran.await.unwrap().unwrap();
    assert!(!unheld.exists(), "a copy of an image no library holds is kept");
}

/// As the server stops, it closes each realtime connection with a Close frame and saves where
/// its account was playing at once, rather than once the requests in progress are answered. It
/// stops the hooks before that, so a plugin enabled by a request still in progress runs nothing.
#[tokio::test]
async fn a_stop_saves_players_and_closes_connections_while_a_request_hangs() {
    let temp = tempfile::tempdir().unwrap();
    let (data, music) = (temp.path().join("data"), temp.path().join("music"));
    fs::create_dir(&music).unwrap();
    let state = DataDir::open(&data, None).unwrap().state().join("jewelcase.db");
    let db = Arc::new(Database::open(&state).unwrap());
    db.write(move |tx| {
        testing::User::new(1, "owner").insert(tx)?;
        add_device(tx, 1, 1, TOKEN);
        testing::library(tx, 1, "Music", &music)?;
        testing::Album::new(101, 1).insert(tx)?;
        testing::Track::new(1, 1, 101).insert(tx)
    })
    .await
    .unwrap();
    // A plugin whose schedule is due as soon as it is enabled, which it is not yet. It has no
    // file, so a delivery fails, and records that it did.
    let mut scheduled = manifest("plugin", &[Schedule]);
    scheduled["permissions"][0]["everyMinutes"] = json!(60);
    install(&db, scheduled).await;
    grant(&crate::plugins::testing::plugins(&temp, &db), "plugin", &[Schedule], &[]).await;
    drop(db);
    // A port free a moment ago, since the test must know it.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let server = run_until(Config { port, ..config(&data, None) }, async {
        stopping.await.unwrap();
    });

    let client = async {
        for _ in 0..100 {
            if TcpStream::connect(address).await.is_ok() {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
        let mut socket = connect_with(address, TOKEN).await;
        start_playing(&mut socket).await;
        report(&mut socket, "playing", json!({ "positionMs": 60_000 })).await;
        // Part of a header holds the drain open, once the server has the connection.
        let mut hanging = TcpStream::connect(address).await.unwrap();
        hanging.write_all(b"GET /api/v1/health HTTP/1.1\r\n").await.unwrap();
        // Enabling the plugin, whose body ends once the stop has begun.
        let mut enabling = TcpStream::connect(address).await.unwrap();
        let head = format!(
            "PUT /api/v1/admin/plugins/plugin/libraries/1 HTTP/1.1\r\nHost: localhost\r\n\
             Authorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\n\
             Content-Length: 16\r\n\r\n{{\"enabled\":"
        );
        enabling.write_all(head.as_bytes()).await.unwrap();
        sleep(Duration::from_millis(200)).await;
        stop.send(()).unwrap();
        let stopped = Instant::now();

        loop {
            match timeout(Duration::from_secs(1), socket.next()).await.unwrap() {
                Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
                other => panic!("closed without a Close frame: {other:?}"),
            }
        }
        let conn = rusqlite::Connection::open(&state).unwrap();
        let saved = "SELECT position_ms FROM player_states WHERE user_id = 1";
        loop {
            let position = conn.query_row(saved, [], |r| r.get::<_, u32>(0)).optional();
            if position.unwrap().is_some_and(|position| position >= 60_000) {
                break;
            }
            assert!(stopped.elapsed() < DRAIN_TIMEOUT / 2, "not saved before the drain");
            sleep(Duration::from_millis(10)).await;
        }
        // The connections were closed after the hooks were stopped, so this wakes the hook loop
        // to a plugin due to run, which it must not start.
        enabling.write_all(b"true}").await.unwrap();
        let mut answer = String::new();
        let answered = timeout(Duration::from_secs(1), enabling.read_to_string(&mut answer));
        answered.await.unwrap().unwrap();
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        hanging
    };
    let (ran, _hanging) = tokio::join!(timeout(DRAIN_TIMEOUT + REFUSED, server), client);
    ran.unwrap().unwrap();
    let conn = rusqlite::Connection::open(&state).unwrap();
    let delivered = "SELECT (SELECT count(*) FROM plugin_cursors) \
                     + (SELECT count(*) FROM plugin_libraries WHERE last_run_at IS NOT NULL)";
    let delivered: i64 = conn.query_row(delivered, [], |row| row.get(0)).unwrap();
    assert_eq!(delivered, 0, "a hook delivery ran after the stop began");
}
