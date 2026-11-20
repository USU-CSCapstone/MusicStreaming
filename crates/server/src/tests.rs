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
use crate::api::testing::player::{KEY, report, start_playing};
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

#[tokio::test]
async fn a_web_build_named_but_not_there_stops_the_server_having_written_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let web_dir = Some(temp.path().join("build"));
    let refused = timeout(REFUSED, run(Config { web_dir, ..config(&data, None) })).await;
    let error = refused.unwrap().unwrap_err();
    assert!(format!("{error:#}").contains(config::WEB_DIR), "{error:#}");
    assert!(!data.exists());
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
        // A queue started just before the stop, so the periodic save has not taken it: only the
        // save the stop ends with can.
        let start = json!({
            "queueId": "11111111-1111-4111-8111-111111111111", "context": { "type": "album" },
            "source": { "key": KEY, "libraryId": "1", "tracks": ["1"] }, "start": 0,
        })
        .to_string();
        let mut starting = TcpStream::connect(address).await.unwrap();
        let request = format!(
            "POST /api/v1/me/queue HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\n\
             Content-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{start}",
            start.len()
        );
        starting.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        timeout(Duration::from_secs(5), starting.read_to_string(&mut answer))
            .await
            .unwrap()
            .unwrap();
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        // Playing the queue's item, which a report must name.
        start_playing(&mut socket).await;
        report(&mut socket, "playing", json!({ "positionMs": 60_000 })).await;
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
        // The connections were closed after the hooks were stopped, so this enables a plugin
        // due to run, which the hook loop must not start.
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
    let queues: i64 =
        conn.query_row("SELECT count(*) FROM queues WHERE active = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(queues, 1, "the queue started just before the stop is saved by it");
}

/// Runs `work` with the logs it writes captured, as the default `info` filter would show them.
async fn logged<T>(work: impl Future<Output = T>) -> (T, String) {
    #[derive(Clone, Default)]
    struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let result = work.await;
    let text = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    (result, text)
}

#[tokio::test]
async fn the_owner_reset_logs_a_live_link_once_and_records_it_without_an_actor() {
    let (temp, db, app) = crate::api::testing::app("/music");
    let before = db::now_ms();
    let (done, log) = logged(reset_owner_password(&db, "/music")).await;
    done.unwrap();

    let lines: Vec<_> = log.lines().filter(|line| line.contains("/music/reset#")).collect();
    assert_eq!(lines.len(), 1, "{log}");
    let code = lines[0]
        .split("/music/reset#")
        .nth(1)
        .unwrap()
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        .next()
        .unwrap();
    assert_eq!(log.matches(code).count(), 1, "{log}");
    assert!(lines[0].contains("WARN") || lines[0].contains("INFO"), "{log}");

    let check = |code: &str| {
        let request = axum::http::Request::post("/music/api/v1/auth/password-resets/check")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(json!({ "code": code }).to_string()))
            .unwrap();
        crate::api::testing::respond(app.clone(), request)
    };
    let (status, _, body) = check(code).await;
    assert_eq!(status, 200, "{body:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        json!({ "username": "owner" })
    );

    let conn = crate::api::testing::conn(&temp);
    let expires: i64 = conn
        .query_row("SELECT reset_expires_at FROM users WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    let lifetime = expires - before;
    assert!((86_400_000..86_400_000 + 60_000).contains(&lifetime), "{lifetime}");
    let audit = "SELECT action, actor_id, target_id FROM admin_actions";
    let rows: Vec<(String, Option<i64>, i64)> = conn
        .prepare(audit)
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows, [("password_reset_issued".to_owned(), None, 1)]);
    // The owner's session is untouched.
    let (status, _) = crate::api::testing::call(&app, TOKEN, "GET", "/music/api/v1/me", None).await;
    assert_eq!(status, 200);

    // Another start replaces the link.
    let (done, again) = logged(reset_owner_password(&db, "/music")).await;
    done.unwrap();
    let newer = again.split("/music/reset#").nth(1).unwrap().split(' ').next().unwrap();
    assert_ne!(newer, code);
    assert_eq!(check(code).await.0, 404);

    let redeem = axum::http::Request::post("/music/api/v1/auth/password-resets/redeem")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            json!({ "code": newer, "password": "vqp-mfrk-ztlw" }).to_string(),
        ))
        .unwrap();
    assert_eq!(crate::api::testing::respond(app.clone(), redeem).await.0, 204);
}

#[tokio::test]
async fn the_owner_reset_before_setup_writes_nothing_and_says_so() {
    let (temp, db, _app) = crate::api::testing::app_before_setup("");
    let (done, log) = logged(reset_owner_password(&db, "")).await;
    done.unwrap();
    assert!(log.contains("no owner"), "{log}");
    assert!(!log.contains("reset#"), "{log}");
    let conn = crate::api::testing::conn(&temp);
    let rows: i64 = conn.query_row("SELECT count(*) FROM admin_actions", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn only_the_setting_makes_a_start_issue_a_link() {
    for owner_reset in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        {
            let data_dir = DataDir::open(&data, None).unwrap();
            let db = Database::open(&data_dir.state().join("jewelcase.db")).unwrap();
            db.write(|tx| testing::User::new(1, "owner").insert(tx)).await.unwrap();
        }
        let config = Config { owner_reset, ..config(&data, None) };
        let server = run_until(config, sleep(Duration::from_millis(300)));
        let (done, log) = logged(timeout(REFUSED, server)).await;
        done.unwrap().unwrap();
        assert_eq!(log.contains("reset#"), owner_reset, "{log}");
        let conn = rusqlite::Connection::open(data.join("state/jewelcase.db")).unwrap();
        let rows = "SELECT (SELECT count(*) FROM admin_actions), \
                           (SELECT count(*) FROM users WHERE reset_hash IS NOT NULL)";
        let rows: (i64, i64) = conn.query_row(rows, [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(rows, (owner_reset.into(), owner_reset.into()));
    }
}
