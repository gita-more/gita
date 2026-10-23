//! The server stops when it is asked to, without losing work.
//!
//! A `SIGTERM` used to kill the process outright: requests that were
//! running were cut off, and the event row for whatever had just
//! happened — written from a background task — never made it to the
//! database. `docker stop` sends exactly that signal.
//!
//! These tests run the real binary and check that it shuts down on
//! `SIGTERM`, and that the event for a request that completed just
//! before the signal is still there afterwards.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The database the test server talks to.
fn test_db_url() -> String {
    std::env::var("GITA_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://gita:gita@127.0.0.1:55432/gita".to_string())
}

/// Create an isolated Postgres schema for a test and return its name.
///
/// The shutdown tests run the real binary, which connects to the *public*
/// schema. Without per-test isolation they share it, and a failed run
/// leaves users/repos/hooks behind — enough state and a subsequent run's
/// `create hook` collides with a leftover and returns 500, which is
/// exactly the intermittent failure this suite has been seeing.
fn create_schema(url: &str, schema: &str) {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async move {
        let pool = sqlx::PgPool::connect(url)
            .await
            .expect("connect to postgres");
        sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS \"{schema}\""))
            .execute(&pool)
            .await
            .expect("create schema");
        pool.close().await;
    });
}

/// Drop a test schema, best-effort, on a separate thread so the test
/// runtime going away cannot cancel it.
fn drop_schema(url: String, schema: String) {
    let worker = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("worker runtime");
        rt.block_on(async move {
            if let Ok(pool) = sqlx::PgPool::connect(&url).await {
                let _ = sqlx::query(&format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE"))
                    .execute(&pool)
                    .await;
                pool.close().await;
            }
        });
    });
    let _ = worker.join();
}

/// A unique per-process schema name.
fn fresh_schema(tag: &str) -> String {
    format!(
        "gita_shutdown_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// A port nothing is listening on.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// The running server, killed on drop so a failing test cannot leave
/// one behind.
struct Server {
    child: Child,
    port: u16,
    schema: String,
    _tmp: tempfile::TempDir,
    log_path: PathBuf,
}

impl Server {
    /// Start the server, isolated in its own Postgres schema, and wait
    /// until it answers.
    fn start() -> Server {
        let tmp = tempfile::TempDir::new().unwrap();
        let port = free_port();
        let log_path = tmp.path().join("server.log");
        let log = std::fs::File::create(&log_path).unwrap();

        let db = test_db_url();
        let schema = fresh_schema("srv");
        create_schema(&db, &schema);

        let config_path = tmp.path().join("gita.toml");
        std::fs::write(
            &config_path,
            format!(
                "[server]\n\
                 host = \"127.0.0.1\"\n\
                 port = {port}\n\
                 data_dir = \"{data}\"\n\
                 database_url = \"{db}\"\n\
                 database_schema = \"{schema}\"\n\
                 jwt_secret = \"test-secret-for-shutdown\"\n",
                data = tmp.path().join("data").display(),
                db = db,
                schema = schema,
            ),
        )
        .unwrap();

        let child = Command::new(env!("CARGO_BIN_EXE_gita-server"))
            .arg("-c")
            .arg(&config_path)
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("the server binary must start");

        let server = Server {
            child,
            port,
            schema,
            _tmp: tmp,
            log_path,
        };
        server.wait_until_ready();
        server
    }

    /// Wait for the server to answer its health check.
    fn wait_until_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(resp) =
                reqwest::blocking::get(format!("http://127.0.0.1:{}/api/health", self.port))
            {
                if resp.status().is_success() {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("the server did not become ready; log:\n{}", self.log());
    }

    /// Ask the server to stop, the way `docker stop` does.
    fn terminate(&self) {
        // `kill -TERM` by hand: libc is not a dependency, and the
        // `nix` crate is not either.
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status()
            .expect("kill must run");
        assert!(status.success(), "kill -TERM failed");
    }

    /// Wait for the process to exit and return its exit code.
    fn wait_for_exit(&mut self) -> Option<i32> {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => return status.code(),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => panic!("could not wait for the server: {e}"),
            }
        }
        None
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // The test may fail before the server is asked to stop; make sure
        // it does not outlive the test.
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Drop the test's schema so it cannot collide with a later run.
        drop_schema(self.db_url(), self.schema.clone());
    }
}

impl Server {
    /// The database URL the test server talks to.
    fn db_url(&self) -> String {
        test_db_url()
    }
}

/// The server shuts down on `SIGTERM` and says so.
#[test]
fn sigterm_stops_the_server_cleanly() {
    let mut server = Server::start();
    server.terminate();

    let code = server
        .wait_for_exit()
        .expect("the server should exit after SIGTERM");
    assert_eq!(code, 0, "the server should exit cleanly");

    let log = server.log();
    assert!(
        log.contains("received SIGTERM"),
        "the server should report the signal it received; log:\n{log}"
    );
    assert!(
        log.contains("Gita stopped"),
        "the server should report a clean stop; log:\n{log}"
    );
}

/// A webhook receiver that takes its time.
///
/// It answers after `delay`, and records that a request arrived. A
/// server that is stopped properly waits for the delivery; one that is
/// killed does not, and the delivery never completes.
struct SlowReceiver {
    port: u16,
    received: Arc<AtomicUsize>,
    _handle: std::thread::JoinHandle<()>,
}

impl SlowReceiver {
    fn start(delay: Duration) -> SlowReceiver {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(AtomicUsize::new(0));
        let counter = received.clone();

        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                counter.fetch_add(1, Ordering::SeqCst);
                // Read whatever the client sent, then take our time.
                let mut buf = [0u8; 1024];
                use std::io::Read;
                let _ = stream.read(&mut buf);
                std::thread::sleep(delay);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                let _ = stream.flush();
            }
        });

        SlowReceiver {
            port,
            received,
            _handle: handle,
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/hook", self.port)
    }

    fn count(&self) -> usize {
        self.received.load(Ordering::SeqCst)
    }
}

/// A request that completed just before the signal still leaves its
/// event behind.
///
/// The event row is written by a background task, so a server that is
/// killed rather than stopped loses it: the push happened, but the
/// event log does not know about it.
#[test]
fn an_event_from_a_request_before_the_signal_survives_the_shutdown() {
    let mut server = Server::start();

    // Register a user and create a repository: both publish events.
    let client = reqwest::blocking::Client::new();
    let base = format!("http://127.0.0.1:{}", server.port);
    let username = format!("shutdown-{}", std::process::id());
    let email = format!("{username}@e2e.local");

    let resp = client
        .post(format!("{base}/api/auth/register"))
        .json(&serde_json::json!({
            "username": username,
            "email": email,
            "password": "hunter22",
        }))
        .send()
        .expect("register");
    assert!(
        resp.status().is_success(),
        "register failed: {}",
        resp.status()
    );
    let token: serde_json::Value = resp.json().expect("register body");
    let token = token["token"].as_str().expect("a token").to_string();

    let repo_name = format!("repo-{}", std::process::id());
    let resp = client
        .post(format!("{base}/api/repos"))
        .header("authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "name": repo_name,
            "description": "",
            "default_branch": "main",
        }))
        .send()
        .expect("create repo");
    assert!(
        resp.status().is_success(),
        "create repo failed: {}",
        resp.status()
    );

    // Now ask the server to stop, straight after the request.
    server.terminate();
    let code = server
        .wait_for_exit()
        .expect("the server should exit after SIGTERM");
    assert_eq!(code, 0, "the server should exit cleanly");

    // The event for the repository that was created must be there.
    // The query runs on its own runtime: `reqwest::blocking` cannot be
    // used from inside an async context, so this test stays synchronous.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime for the query");
    // The event lives in the server's own schema, so the table has to be
    // qualified — an unqualified `events` would read `public.events`,
    // which this test never wrote to.
    // Quote the schema alone; the dot has to stay OUTSIDE the quotes, or
    // Postgres reads the whole thing as one identifier named
    // "schema.events".
    let events_table = format!("\"{}\".events", server.schema);
    let schema_for_debug = server.schema.clone();
    let log_for_debug = server.log();
    let rows: Vec<(String,)> = rt.block_on(async {
        let pool = sqlx::PgPool::connect(&test_db_url())
            .await
            .expect("connect to the database");
        // What did the server actually build in its schema?
        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = $1 ORDER BY 1",
        )
        .bind(&schema_for_debug)
        .fetch_all(&pool)
        .await
        .expect("list schema tables");
        let rows = sqlx::query_as(&format!(
            "SELECT event_type FROM {events_table} WHERE payload::json->>'repo' = $1 ORDER BY id"
        ))
        .bind(&repo_name)
        .fetch_all(&pool)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "query the events ({e}); schema tables: {tables:?};\nserver log:\n{log_for_debug}"
            )
        });
        pool.close().await;
        rows
    });

    assert!(
        rows.iter().any(|(t,)| t == "repo_created"),
        "the event for {repo_name} should have survived the shutdown; got {rows:?}"
    );
}

/// The server waits for a webhook delivery that is still in flight.
///
/// The delivery is a background task, so a server that is killed rather
/// than stopped drops it: the webhook was registered, the event
/// happened, and the receiver never heard about it. A server that stops
/// properly waits for the delivery to finish before it exits.
///
/// The receiver here takes two seconds to answer, so the difference is
/// unmistakable: without the wait, the server is gone in milliseconds.
#[test]
fn the_shutdown_waits_for_a_webhook_delivery_in_flight() {
    let receiver = SlowReceiver::start(Duration::from_secs(2));
    let mut server = Server::start();

    let client = reqwest::blocking::Client::new();
    let base = format!("http://127.0.0.1:{}", server.port);
    let username = format!("hookshutdown-{}", std::process::id());

    let resp = client
        .post(format!("{base}/api/auth/register"))
        .json(&serde_json::json!({
            "username": username,
            "email": format!("{username}@e2e.local"),
            "password": "hunter22",
        }))
        .send()
        .expect("register");
    assert!(
        resp.status().is_success(),
        "register failed: {}",
        resp.status()
    );
    let token: serde_json::Value = resp.json().expect("register body");
    let token = token["token"].as_str().expect("a token").to_string();

    // Register a webhook that points at the slow receiver. It listens
    // for the event that creating a repository produces — a webhook
    // only hears about the repository it belongs to.
    let repo_name = format!("hookrepo-{}", std::process::id());
    let resp = client
        .post(format!("{base}/api/repos"))
        .header("authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "name": repo_name,
            "description": "",
            "default_branch": "main",
        }))
        .send()
        .expect("create repo");
    assert!(resp.status().is_success(), "create repo: {}", resp.status());

    let resp = client
        .post(format!("{base}/api/repos/{username}/{repo_name}/hooks"))
        .header("authorization", format!("Bearer {token}"))
        // A webhook listens for `push` by default; this one wants the
        // event that opening an issue produces.
        .json(&serde_json::json!({
            "url": receiver.url(),
            "events": ["issue_created"],
        }))
        .send()
        .expect("create hook");
    assert!(
        resp.status().is_success(),
        "create hook: {}\nserver log:\n{}",
        resp.status(),
        server.log()
    );

    // An issue in that repository: the event for it fans out to the
    // webhook. A webhook only hears about the repository it belongs to,
    // so the issue has to be in the repository the hook is on.
    let resp = client
        .post(format!("{base}/api/repos/{username}/{repo_name}/issues"))
        .header("authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({"title": "trigger the webhook", "body": ""}))
        .send()
        .expect("create issue");
    assert!(
        resp.status().is_success(),
        "create issue: {}",
        resp.status()
    );

    // The delivery has started. Ask the server to stop while it is still
    // in flight.
    //
    // The deadline is generous on purpose: this test runs in the same
    // suite as the whole database-backed integration suite, and under
    // that load the webhook dispatcher can take a while to get going.
    let deadline = Instant::now() + Duration::from_secs(30);
    while receiver.count() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    // "At least one delivery" rather than "exactly one": the dispatcher
    // may retry a delivery whose response was slow, and a retry is not
    // what this test is about.
    assert!(
        receiver.count() >= 1,
        "the webhook should have been delivered"
    );

    let started = Instant::now();
    server.terminate();
    let code = server
        .wait_for_exit()
        .expect("the server should exit after SIGTERM");
    let took = started.elapsed();

    assert_eq!(code, 0, "the server should exit cleanly");
    assert!(
        took >= Duration::from_millis(1500),
        "the server exited after {took:?}, but the webhook delivery had not \
         finished — it did not wait for the background task"
    );
}
