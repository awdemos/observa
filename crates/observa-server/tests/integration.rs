use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::{routing::post, Json, Router};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tokio_stream::StreamExt;
use uuid::Uuid;

use observa_bus::Bus;
use observa_cache::Cache;
use observa_config::Config;
use observa_db::Db;
use observa_ingestor::{spawn_ingestor, IngestorOpts, LogSource};
use observa_server::{router, AppState};
use observa_shared::Event;

const COLLECTOR_INTERVAL_MS: u64 = 200;
const SSE_TIMEOUT_MS: u64 = 5_000;
const SHUTDOWN_TIMEOUT_MS: u64 = 5_000;

/// A running test application: server + collector + ingestor + shutdown controls.
struct TestApp {
    base_url: String,
    db_path: String,
    server_shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    collector_shutdown: watch::Sender<bool>,
    ingestor_shutdown: watch::Sender<bool>,
    server_handle: JoinHandle<()>,
    collector_handle: JoinHandle<()>,
    ingestor_handle: JoinHandle<()>,
}

impl TestApp {
    /// Send shutdown signals and wait for all tasks to finish.
    async fn shutdown(mut self) {
        let _ = self
            .server_shutdown
            .take()
            .expect("server shutdown sender present")
            .send(());
        let _ = self.collector_shutdown.send(true);
        let _ = self.ingestor_shutdown.send(true);

        let mut handles = [
            Some(self.server_handle),
            Some(self.collector_handle),
            Some(self.ingestor_handle),
        ];

        let result = timeout(Duration::from_millis(SHUTDOWN_TIMEOUT_MS), async {
            for opt in &mut handles {
                if let Some(handle) = opt.take() {
                    let _ = handle.await;
                }
            }
        })
        .await;

        if result.is_err() {
            tracing::warn!("integration shutdown timed out; aborting tasks");
            for opt in &mut handles {
                if let Some(handle) = opt.take() {
                    handle.abort();
                }
            }
        }

        let _ = std::fs::remove_file(&self.db_path);
        let _ = std::fs::remove_file(format!("{}-wal", self.db_path));
        let _ = std::fs::remove_file(format!("{}-shm", self.db_path));
    }
}

/// Spawn a mock OpenAI-compatible server and return its base URL.
async fn mock_llm_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock llm should bind");
    let addr = listener.local_addr().expect("mock llm should have address");

    tokio::spawn(async move {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|Json(body): Json<serde_json::Value>| async move {
                let echo = body
                    .get("messages")
                    .and_then(|m| m.as_array())
                    .and_then(|m| m.last())
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("integration reply");
                Json(json!({
                    "choices": [{"message": {"role": "assistant", "content": format!("Echo: {echo}")}}]
                }))
            }),
        );
        let _ = axum::serve(listener, app).await;
    });

    sleep(Duration::from_millis(50)).await;
    format!("http://{addr}/v1")
}

async fn spawn_test_app() -> TestApp {
    let db_id = Uuid::new_v4();
    let db_path = format!("/tmp/observa_integration_{db_id}.db");
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(format!("{db_path}-wal"));
    let _ = std::fs::remove_file(format!("{db_path}-shm"));

    let db = Db::new(&format!("sqlite://{db_path}"))
        .await
        .expect("integration db should build");
    let bus = Bus::new();
    let cache = Cache::new(None).await.expect("degraded cache should build");

    let llm_api_base = mock_llm_url().await;
    let config = Config {
        llm_api_base,
        llm_api_key: Some("test-key".to_string()),
        database_url: None,
        redis_url: None,
        ..Default::default()
    };

    let state = Arc::new(
        AppState::new(config, bus.clone(), Some(db.clone()), Some(cache))
            .expect("app state should build"),
    );

    // Server
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("server should bind");
    let addr = listener.local_addr().expect("server should have address");
    let (server_shutdown_tx, server_shutdown_rx) = tokio::sync::oneshot::channel();
    let serve = axum::serve(listener, router(state)).with_graceful_shutdown(async {
        let _ = server_shutdown_rx.await;
    });
    let server_handle = tokio::spawn(async move {
        if let Err(error) = serve.await {
            tracing::error!(%error, "integration server error");
        }
    });

    // Collector
    let (collector_shutdown_tx, collector_shutdown_rx) = watch::channel(false);
    let collector_inner = observa_collector::spawn_collector(observa_collector::CollectorOpts {
        interval_ms: COLLECTOR_INTERVAL_MS,
        db: Some(db.clone()),
        cache: None,
        bus: bus.clone(),
        shutdown: collector_shutdown_rx,
        compression_enabled: true,
        ai_server_endpoints: Vec::new(),
        ai_server_subnet_scan: false,
    });
    let collector_handle = tokio::spawn(async move {
        let _ = collector_inner.await;
    });

    // Ingestor reads a temp log file
    let log_id = Uuid::new_v4();
    let log_path: PathBuf = format!("/tmp/observa_integration_{log_id}.log").into();
    std::fs::write(&log_path, "integration fallback log line\n")
        .expect("temp log should be writable");
    let (ingestor_shutdown_tx, ingestor_shutdown_rx) = watch::channel(false);
    let ingestor_inner = spawn_ingestor(IngestorOpts {
        source: LogSource::File { path: log_path },
        tail: true,
        db: Some(db),
        cache: None,
        bus,
        shutdown: ingestor_shutdown_rx,
    });
    let ingestor_handle = tokio::spawn(async move {
        let _ = ingestor_inner.await;
    });

    sleep(Duration::from_millis(100)).await;

    TestApp {
        base_url: format!("http://{addr}"),
        db_path,
        server_shutdown: Some(server_shutdown_tx),
        collector_shutdown: collector_shutdown_tx,
        ingestor_shutdown: ingestor_shutdown_tx,
        server_handle,
        collector_handle,
        ingestor_handle,
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("http client should build")
}

#[tokio::test]
async fn qa_1_dashboard_renders_without_js() {
    let app = spawn_test_app().await;
    let client = http_client();

    let response = client
        .get(format!("{}/", app.base_url))
        .send()
        .await
        .expect("dashboard request should succeed");

    let status = response.status();
    let body = response.text().await.expect("body should be readable");
    eprintln!("dashboard status: {}, body: {}", status, body);
    assert_eq!(status, 200);
    assert!(body.contains("href=\"/metrics\""), "missing metrics link");
    assert!(body.contains("href=\"/logs\""), "missing logs link");
    assert!(body.contains("href=\"/chat\""), "missing chat link");

    app.shutdown().await;
}

#[tokio::test]
async fn qa_2_metrics_sse_stream_receives_event() {
    let app = spawn_test_app().await;
    let client = http_client();

    let response = client
        .get(format!("{}/events", app.base_url))
        .send()
        .await
        .expect("sse request should succeed");
    assert_eq!(response.status(), 200);

    let mut stream = response.bytes_stream();
    let found = timeout(Duration::from_millis(SSE_TIMEOUT_MS), async {
        let mut buffer = String::new();
        while let Some(chunk) = stream.next().await {
            if let Ok(bytes) = chunk {
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                // SSE events are separated by a blank line. Collect complete
                // events and return the first metric event. Other events (logs,
                // heartbeats) may arrive first now that the collector also
                // probes AI inference servers before publishing.
                loop {
                    let split_pos = buffer.find("\n\n");
                    if split_pos.is_none() {
                        break;
                    }
                    let pos = split_pos.unwrap();
                    let event = buffer[..pos].to_string();
                    buffer = buffer[pos + 2..].to_string();
                    if let Some(line) = event.lines().find(|l| l.starts_with("data:")) {
                        let payload = line.strip_prefix("data: ").unwrap_or(line);
                        if let Ok(event) = serde_json::from_str::<Event>(payload) {
                            if event.is_metric() {
                                return line.to_string();
                            }
                        }
                    }
                }
            }
        }
        String::new()
    })
    .await
    .unwrap_or_default();

    assert!(!found.is_empty(), "timed out waiting for a metric SSE event");

    app.shutdown().await;
}

#[tokio::test]
async fn qa_3_ingestor_fallback_reads_log_file() {
    let app = spawn_test_app().await;
    let client = http_client();

    let found = timeout(Duration::from_millis(SSE_TIMEOUT_MS), async {
        let mut checks = 0;
        loop {
            checks += 1;
            if checks > 50 {
                return false;
            }
            let response = client
                .get(format!("{}/api/logs/history", app.base_url))
                .send()
                .await
                .expect("logs history request should succeed");
            let logs: Vec<observa_shared::LogEvent> =
                response.json().await.expect("logs history should be json");
            if logs
                .iter()
                .any(|l| l.message.contains("integration fallback"))
            {
                return true;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or(false);

    assert!(
        found,
        "ingestor should have read the fallback log file within timeout"
    );

    app.shutdown().await;
}

#[tokio::test]
async fn qa_4_chat_ask_stores_session_and_returns_reply() {
    let app = spawn_test_app().await;
    let client = http_client();

    let session = client
        .post(format!("{}/api/chat/session", app.base_url))
        .send()
        .await
        .expect("session request should succeed")
        .json::<serde_json::Value>()
        .await
        .expect("session response should be json");
    let session_id = session
        .get("session_id")
        .and_then(|v| v.as_str())
        .expect("session_id should be present");
    let owner_token = session
        .get("owner_token")
        .and_then(|v| v.as_str())
        .expect("owner_token should be present");

    let ask = client
        .post(format!("{}/api/chat/ask", app.base_url))
        .json(&json!({
            "session_id": session_id,
            "owner_token": owner_token,
            "message": "ping"
        }))
        .send()
        .await
        .expect("ask request should succeed");

    assert_eq!(ask.status(), 200);
    let reply = ask
        .json::<serde_json::Value>()
        .await
        .expect("ask response should be json");
    let content = reply
        .get("reply")
        .and_then(|v| v.as_str())
        .expect("reply should be present");
    assert!(
        content.contains("Echo: ping"),
        "unexpected reply: {content}"
    );

    // Verify the exchange was persisted by reading via the chat partial.
    let partial = client
        .get(format!(
            "{}/partials/chat?session_id={session_id}&owner_token={owner_token}",
            app.base_url
        ))
        .send()
        .await
        .expect("chat partial request should succeed");
    assert_eq!(partial.status(), 200);
    let html = partial.text().await.expect("partial should be readable");
    assert!(html.contains("Echo: ping"), "chat partial missing reply");
    assert!(html.contains("ping"), "chat partial missing user message");

    app.shutdown().await;
}

#[tokio::test]
async fn qa_5_graceful_shutdown_completes_within_timeout() {
    let app = spawn_test_app().await;

    let shutdown_start = tokio::time::Instant::now();
    app.shutdown().await;
    let elapsed = shutdown_start.elapsed();

    assert!(
        elapsed < Duration::from_millis(SHUTDOWN_TIMEOUT_MS),
        "shutdown took too long: {elapsed:?}"
    );
}
