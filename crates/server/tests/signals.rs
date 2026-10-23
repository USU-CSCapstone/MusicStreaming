//! Stopping the server's binary with signals.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// The server's process, killed if a test ends before it exits.
struct Server(Child);

impl Server {
    /// Starts the server over an empty data directory in `data`, listening on `port`, with no
    /// other setting than the `PATH` it finds ffmpeg on.
    fn start(data: &std::path::Path, port: u16) -> Server {
        let child = Command::new(env!("CARGO_BIN_EXE_jewelcase-server"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("JEWELCASE_DATA_DIR", data)
            .env("JEWELCASE_PORT", port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Server(child)
    }

    /// Sends it `signal`, such as `TERM`.
    fn signal(&self, signal: &str) {
        let pid = self.0.id().to_string();
        let sent = Command::new("kill").arg(format!("-{signal}")).arg(pid).status().unwrap();
        assert!(sent.success());
    }

    /// Its exit code, waiting at most `limit` for it to exit: `None` if it has not exited by
    /// then, or was ended by a signal.
    fn exit_code(&mut self, limit: Duration) -> Option<i32> {
        let started = Instant::now();
        while started.elapsed() < limit {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status.code();
            }
            sleep(Duration::from_millis(20));
        }
        None
    }
}

impl Drop for Server {
    /// Kills the process if it is still running, and reaps it.
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}

/// Waits until the server on `port` answers a request, which it does only once it listens for
/// signals. The deadline only bounds a hang.
fn serving(port: u16) {
    let started = Instant::now();
    loop {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: jewelcase\r\n\r\n").unwrap();
            let mut response = String::new();
            if stream.read_to_string(&mut response).is_ok() && response.starts_with("HTTP/1.1 200")
            {
                return;
            }
        }
        assert!(started.elapsed() < Duration::from_secs(30), "the server did not start");
        sleep(Duration::from_millis(50));
    }
}

/// A second signal while the server waits for a request in progress ends it at once, with exit
/// code 130, rather than once the wait is over.
#[test]
fn a_second_signal_stops_the_server_at_once() {
    let data = tempfile::tempdir().unwrap();
    // A port free a moment ago, since the test must know it.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut server = Server::start(data.path(), port);
    serving(port);
    // Part of a header holds the stop open for its drain timeout, 3 s.
    let mut hanging = TcpStream::connect(("127.0.0.1", port)).unwrap();
    hanging.write_all(b"GET /api/v1/health HTTP/1.1\r\n").unwrap();
    sleep(Duration::from_millis(200));

    server.signal("TERM");
    assert_eq!(server.exit_code(Duration::from_millis(500)), None, "stopped on the first");
    server.signal("INT");
    assert_eq!(server.exit_code(Duration::from_secs(1)), Some(130));
}
