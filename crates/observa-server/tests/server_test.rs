use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use axum::{body::Body, routing::post, Json, Router};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use observa_bus::Bus;
use observa_config::Config;
use observa_server::{router, AppState};
use observa_server::store::{InMemoryChatStore, InMemoryStore};
use observa_shared::{Event, HealthStatus, HeartbeatEvent, LogEvent, Role, SecurityAlert, Severity};

static DB_COUNTER: AtomicU64 = AtomicU64::new(0);
static CHAT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn owner_token() -> String {
    "test-owner-token".to_string()
}

async fn test_state_with_db() -> Arc<AppState> {
    let n = DB_COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = format!("/tmp/observa_server_db_test_{n}.db");
    let _ = std::fs::remove_file(&path);
    let url = format!("sqlite://{path}");
    let db = observa_db::Db::new(&url)
        .await
        .expect("test database should be created");
    Arc::new(
        AppState::new(Config::default(), Bus::new(), Some(db), None)
            .expect("state should build with test db"),
    )
}

async fn test_state_with_metric() -> Arc<AppState> {
    let state = test_state_with_db().await;
    let db = state.db.as_ref().expect("test db should be present");
    let snapshot = observa_shared::MetricSnapshot {
        ts: chrono::Utc::now(),
        cpu: observa_shared::CpuMetrics {
            usage_percent: 12.5,
            per_core_usage: vec![10.0, 15.0],
            frequency_mhz: 2800,
        },
        memory: observa_shared::MemoryMetrics {
            total_bytes: 16_000_000_000,
            used_bytes: 8_000_000_000,
            free_bytes: 8_000_000_000,
        },
        disks: vec![observa_shared::DiskMetrics {
            name: "/dev/sda1".into(),
            total_bytes: 500_000_000_000,
            used_bytes: 100_000_000_000,
            read_bytes_per_sec: 1_000_000.0,
            write_bytes_per_sec: 500_000.0,
        }],
        networks: vec![observa_shared::NetworkMetrics {
            interface: "eth0".into(),
            rx_bytes: 1_000_000,
            tx_bytes: 2_000_000,
            rx_rate: 0.0,
            tx_rate: 0.0,
        }],
        processes: vec![observa_shared::ProcessMetrics {
            pid: 1,
            name: "init".into(),
            cmdline: None,
            cpu_percent: 5.0,
            memory_bytes: 100_000_000,
        }],
        gpu: vec![],
        swap: None,
        ai_servers: vec![],
    };
    observa_db::metrics::store(db, &snapshot, true)
        .await
        .expect("metric snapshot should store");
    state
}

fn sample_metric() -> observa_shared::MetricSnapshot {
    observa_shared::MetricSnapshot {
        ts: chrono::Utc::now(),
        cpu: observa_shared::CpuMetrics {
            usage_percent: 12.5,
            per_core_usage: vec![10.0, 15.0],
            frequency_mhz: 2800,
        },
        memory: observa_shared::MemoryMetrics {
            total_bytes: 16_000_000_000,
            used_bytes: 8_000_000_000,
            free_bytes: 8_000_000_000,
        },
        disks: vec![observa_shared::DiskMetrics {
            name: "/dev/sda1".into(),
            total_bytes: 500_000_000_000,
            used_bytes: 100_000_000_000,
            read_bytes_per_sec: 1_000_000.0,
            write_bytes_per_sec: 500_000.0,
        }],
        networks: vec![observa_shared::NetworkMetrics {
            interface: "eth0".into(),
            rx_bytes: 1_000_000,
            tx_bytes: 2_000_000,
            rx_rate: 0.0,
            tx_rate: 0.0,
        }],
        processes: vec![observa_shared::ProcessMetrics {
            pid: 1,
            name: "init".into(),
            cmdline: None,
            cpu_percent: 5.0,
            memory_bytes: 100_000_000,
        }],
        gpu: vec![],
        swap: None,
        ai_servers: vec![],
    }
}

async fn test_state_with_in_memory_metric() -> Arc<AppState> {
    let mut state =
        AppState::new(Config::default(), Bus::new(), None, None).expect("state should build");
    let memory = InMemoryStore::new(10);
    memory.push_metric(sample_metric()).await;
    state.store = Arc::new(memory);
    state.chat_store = Arc::new(InMemoryChatStore::new(100));
    Arc::new(state)
}

fn test_state() -> Arc<AppState> {
    Arc::new(AppState::new(Config::default(), Bus::new(), None, None).expect("state should build"))
}

fn state_without_fallback() -> Arc<AppState> {
    let config = Config {
        llm_api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    let mut state = AppState::new(config, Bus::new(), None, None).expect("state should build");
    state.fallback = None;
    state.llm = None;
    Arc::new(state)
}

async fn body_string(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn mock_llm_app() -> Router {
    Router::new().route("/v1/chat/completions", post(mock_complete))
}

async fn mock_complete(Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
    let echo = body
        .get("messages")
        .and_then(|m| m.as_array())
        .and_then(|m| m.last())
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("mock reply");
    Json(json!({
        "choices": [{"message": {"role": "assistant", "content": format!("Echo: {echo}")}}]
    }))
}

fn mock_slow_llm_app() -> Router {
    Router::new().route("/v1/chat/completions", post(mock_slow_complete))
}

async fn mock_slow_complete(Json(_body): Json<serde_json::Value>) -> Json<serde_json::Value> {
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    Json(json!({
        "choices": [{"message": {"role": "assistant", "content": "too late"}}]
    }))
}

async fn mock_slow_llm_url() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock slow llm should bind");
    let addr = listener.local_addr().expect("mock slow llm should have address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, mock_slow_llm_app()).await;
    });
    format!("http://{addr}/v1")
}

async fn mock_llm_url() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock llm should bind");
    let addr = listener.local_addr().expect("mock llm should have address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, mock_llm_app()).await;
    });
    format!("http://{addr}/v1")
}

fn mock_alarmist_llm_app() -> Router {
    Router::new().route("/v1/chat/completions", post(mock_alarmist_complete))
}

async fn mock_alarmist_complete(Json(_body): Json<serde_json::Value>) -> Json<serde_json::Value> {
    Json(json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Critical failure! The system is on fire and everything is unhealthy."
            }
        }]
    }))
}

async fn mock_alarmist_llm_url() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock alarmist llm should bind");
    let addr = listener
        .local_addr()
        .expect("mock alarmist llm should have address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, mock_alarmist_llm_app()).await;
    });
    format!("http://{addr}/v1")
}

#[tokio::test]
async fn dashboard_renders_navigation() {
    let app = router(test_state());
    let response = app
        .oneshot(axum::http::Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(body.contains("href=\"/metrics\""), "missing metrics link");
    assert!(body.contains("href=\"/logs\""), "missing logs link");
    assert!(body.contains("href=\"/chat\""), "missing chat link");
}

#[tokio::test]
async fn chat_session_without_db_returns_uuid() {
    let app = router(test_state());
    let response = app
        .oneshot(
            axum::http::Request::post("/api/chat/session")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body should be json");
    let session_id = parsed
        .get("session_id")
        .and_then(|v| v.as_str())
        .expect("session_id should be present");
    assert!(
        Uuid::parse_str(session_id).is_ok(),
        "session_id should be a uuid"
    );
    assert!(
        parsed
            .get("owner_token")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "owner_token should be present"
    );
}

#[tokio::test]
async fn chat_ask_without_llm_uses_fallback_responder() {
    let _lock = CHAT_TEST_LOCK.lock().await;
    let state = test_state();
    state.clear_all_rate_limiters().await;
    let mut rx = state.bus.subscribe();
    let app = router(Arc::clone(&state));
    let payload = json!({
        "session_id": Uuid::new_v4(),
        "owner_token": owner_token(),
        "message": "hello"
    })
    .to_string();
    let response = app
        .oneshot(
            axum::http::Request::post("/api/chat/ask")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body should be json");
    let reply = parsed
        .get("reply")
        .and_then(|v| v.as_str())
        .expect("reply should be present");
    assert!(
        reply.contains("Hello. I'm Observa"),
        "fallback greeting missing: {reply}"
    );

    let event = rx.recv().await.expect("chat event should be published");
    match event {
        Event::Chat(msg) => {
            assert_eq!(msg.role, Role::Assistant);
            assert!(
                msg.content.contains("Hello. I'm Observa"),
                "published chat content missing greeting: {msg:?}"
            );
        }
        other => panic!("expected Event::Chat, got {other:?}"),
    }
}

#[tokio::test]
async fn chat_ask_with_neither_llm_nor_fallback_returns_unprocessable_entity() {
    let _lock = CHAT_TEST_LOCK.lock().await;
    let state = state_without_fallback();
    state.clear_all_rate_limiters().await;
    let app = router(state);
    let payload = json!({
        "session_id": Uuid::new_v4(),
        "owner_token": owner_token(),
        "message": "hello"
    })
    .to_string();
    let response = app
        .oneshot(
            axum::http::Request::post("/api/chat/ask")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn chat_stream_without_llm_uses_fallback_responder() {
    let state = test_state();
    let mut rx = state.bus.subscribe();
    let app = router(Arc::clone(&state));
    let response = app
        .oneshot(
            axum::http::Request::get("/api/chat/stream")
                .uri(format!(
                    "/api/chat/stream?session_id={}&owner_token={}&message=hello",
                    Uuid::new_v4(),
                    owner_token()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(
        body.contains("Hello.") && body.contains("Observa"),
        "fallback stream greeting missing: {body}"
    );

    let event = rx.recv().await.expect("chat event should be published");
    match event {
        Event::Chat(msg) => {
            assert_eq!(msg.role, Role::Assistant);
            assert!(
                msg.content.contains("Hello.") && msg.content.contains("Observa"),
                "published chat content missing greeting: {msg:?}"
            );
        }
        other => panic!("expected Event::Chat, got {other:?}"),
    }
}

#[tokio::test]
async fn chat_stream_with_neither_llm_nor_fallback_returns_unprocessable_entity() {
    let app = router(state_without_fallback());
    let response = app
        .oneshot(
            axum::http::Request::get("/api/chat/stream")
                .uri(format!(
                    "/api/chat/stream?session_id={}&owner_token={}&message=hello",
                    Uuid::new_v4(),
                    owner_token()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn assets_serve_vendor_and_app_files() {
    let app = router(test_state());

    let css = app
        .clone()
        .oneshot(
            axum::http::Request::get("/assets/css/observa.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(css.status(), 200);
    let css_body = body_string(css).await;
    assert!(
        css_body.contains("--bg:"),
        "css should contain theme variables"
    );

    let js = app
        .clone()
        .oneshot(
            axum::http::Request::get("/assets/js/observa.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(js.status(), 200);
    let js_body = body_string(js).await;
    assert!(
        js_body.contains("EventSource"),
        "js should contain sse wiring"
    );

    let three = app
        .clone()
        .oneshot(
            axum::http::Request::get("/assets/vendor/three/three.min.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(three.status(), 200);

    let controls = app
        .oneshot(
            axum::http::Request::get("/assets/vendor/three/OrbitControls.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(controls.status(), 200);
}

#[tokio::test]
async fn chat_ask_with_mock_llm_returns_reply() {
    let _lock = CHAT_TEST_LOCK.lock().await;
    let config = Config {
        llm_api_base: mock_llm_url().await,
        llm_api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    let state = Arc::new(
        AppState::new(config, Bus::new(), None, None).expect("state should build with mock llm"),
    );
    state.clear_all_rate_limiters().await;
    let app = router(state);

    let payload = json!({
        "session_id": Uuid::new_v4(),
        "owner_token": owner_token(),
        "message": "ping"
    })
    .to_string();
    let response = app
        .oneshot(
            axum::http::Request::post("/api/chat/ask")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body should be json");
    let reply = parsed
        .get("reply")
        .and_then(|v| v.as_str())
        .expect("reply should be present");
    assert!(reply.contains("Echo: ping"), "unexpected reply: {reply}");
}

#[tokio::test]
async fn chat_ask_html_with_slow_llm_returns_timeout_error() {
    std::env::set_var("OBSERVA_CHAT_TIMEOUT_SECS", "1");
    let _lock = CHAT_TEST_LOCK.lock().await;
    let config = Config {
        llm_api_base: mock_slow_llm_url().await,
        llm_api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    let state = Arc::new(
        AppState::new(config, Bus::new(), None, None)
            .expect("state should build with slow mock llm"),
    );
    state.clear_all_rate_limiters().await;
    let app = router(state);

    let payload = json!({
        "session_id": Uuid::new_v4(),
        "owner_token": owner_token(),
        "message": "hello"
    })
    .to_string();

    let response = app
        .oneshot(
            axum::http::Request::post("/api/chat/ask-html")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(
        body.contains("Error:") && body.contains("timed out"),
        "expected rendered timeout error, got: {body}"
    );
}

#[tokio::test]
async fn log_filter_partial_filters_by_query_and_severity() {
    let state = test_state_with_db().await;
    let db = state.db.as_ref().expect("test db should be present");

    let error_log = LogEvent {
        ts: chrono::Utc::now(),
        source: "journald".into(),
        unit: None,
        severity: Severity::Error,
        message: "disk full on /dev/sda1".into(),
        raw: None,
        security: false,
    };
    let info_log = LogEvent {
        ts: chrono::Utc::now(),
        source: "journald".into(),
        unit: None,
        severity: Severity::Info,
        message: "started nginx service".into(),
        raw: None,
        security: false,
    };

    observa_db::logs::store(db, &error_log)
        .await
        .expect("store error log failed");
    observa_db::logs::store(db, &info_log)
        .await
        .expect("store info log failed");

    let app = router(state);
    let response = app
        .oneshot(
            axum::http::Request::get("/partials/logs?q=disk&severity=error")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(
        body.contains("disk full on"),
        "body should include error log: {body}"
    );
    assert!(
        !body.contains("started nginx service"),
        "body should not include info log: {body}"
    );
}

#[tokio::test]
async fn metrics_page_renders_summary_cards() {
    let state = test_state_with_metric().await;
    let app = router(state);
    let response = app
        .oneshot(axum::http::Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(body.contains("Metrics"), "page title missing");
    assert!(body.contains("id=\"metrics-kpi-strip\""), "KPI strip missing");
    assert!(body.contains("id=\"processes-table\""), "processes table missing");
}

#[tokio::test]
async fn dashboard_renders_mission_control_layout() {
    let state = test_state_with_metric().await;
    let app = router(state);
    let response = app
        .oneshot(axum::http::Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(body.contains("id=\"kpi-strip\""), "KPI strip missing");
    assert!(body.contains("class=\"dashboard-grid\""), "dashboard grid missing");
    assert!(body.contains("id=\"dashboard-process-cards\""), "dashboard process cards missing");
    assert!(body.contains("id=\"dashboard-network-cards\""), "dashboard network cards missing");
    assert!(body.contains("id=\"dashboard-storage-cards\""), "dashboard storage cards missing");
    assert!(body.contains("id=\"security-rows\""), "security rows missing");
    assert!(body.contains("id=\"logs-panel\""), "logs panel missing");
    assert!(body.contains("class=\"process-card\""), "process card missing");
    assert!(body.contains("class=\"network-card\""), "network card missing");
    assert!(body.contains("class=\"storage-card\""), "storage card missing");
}

#[tokio::test]
async fn dashboard_renders_summary_from_in_memory_store() {
    let state = test_state_with_in_memory_metric().await;
    let app = router(state);
    let response = app
        .oneshot(axum::http::Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(body.contains("Metrics"), "page title missing");
    assert!(body.contains("id=\"metrics-kpi-strip\""), "KPI strip missing");
    assert!(body.contains("CPU"), "cpu card missing");
    assert!(body.contains("Memory"), "memory card missing");
}

#[tokio::test]
async fn logs_page_renders_filter_form() {
    let state = test_state_with_db().await;
    let app = router(state);
    let response = app
        .oneshot(axum::http::Request::get("/logs").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(body.contains("id=\"log-filter\""), "filter form missing");
    assert!(body.contains("id=\"logs-panel\""), "logs panel missing");
}

#[tokio::test]
async fn logs_page_truncates_overlong_query() {
    let state = test_state_with_db().await;
    let app = router(state);
    let q = "x".repeat(300);
    let response = app
        .oneshot(
            axum::http::Request::get(format!("/logs?q={}", q))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(!body.contains(&"x".repeat(250)), "query should be truncated");
}

#[tokio::test]
async fn logs_page_ignores_unknown_severity_values() {
    let state = test_state_with_db().await;
    let app = router(state);
    let response = app
        .oneshot(
            axum::http::Request::get("/logs?severity=info&severity=bogus&severity=critical")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(!body.contains("<script>"), "invalid severity should be stripped");
    assert!(body.contains("info") || body.contains("critical"), "valid severities preserved");
}

#[tokio::test]
async fn security_page_renders_alert_table() {
    let state = test_state_with_db().await;
    let app = router(state);
    let response = app
        .oneshot(axum::http::Request::get("/security").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(body.contains("id=\"security-rows\""), "security rows missing");
    assert!(body.contains("id=\"security-data\""), "security data JSON missing");
}

#[tokio::test]
async fn network_page_renders_panel() {
    let state = test_state();
    let app = router(state);
    let response = app
        .oneshot(axum::http::Request::get("/network").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(
        body.contains("class=\"network-grid\"") || body.contains("No network interfaces detected"),
        "network grid or empty state missing"
    );
    assert!(body.contains("id=\"network-data\""), "network data JSON missing");
}

#[tokio::test]
async fn processes_page_renders_cards() {
    let mut state = AppState::new(Config::default(), Bus::new(), None, None).expect("state should build");
    let memory = InMemoryStore::new(10);
    memory.push_metric(sample_metric()).await;
    memory.push_log(LogEvent {
        ts: chrono::Utc::now(),
        source: "kernel".into(),
        unit: None,
        severity: Severity::Warn,
        message: "high CPU usage in worker".into(),
        raw: None,
        security: false,
    }).await;
    state.store = Arc::new(memory);
    state.chat_store = Arc::new(InMemoryChatStore::new(100));
    let app = router(Arc::new(state));
    let response = app
        .oneshot(axum::http::Request::get("/processes").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(body.contains("class=\"process-grid\""), "process grid missing");
    assert!(body.contains("init"), "process name missing");
    assert!(body.contains("Recent Process Events"), "process events heading missing");
    assert!(body.contains("high CPU usage in worker"), "process event row missing");
    assert!(body.contains("class=\"process-sparkline\""), "process sparkline missing");
}

#[tokio::test]
async fn storage_page_renders_cards() {
    let mut state = AppState::new(Config::default(), Bus::new(), None, None).expect("state should build");
    let memory = InMemoryStore::new(10);
    memory.push_metric(sample_metric()).await;
    memory.push_log(LogEvent {
        ts: chrono::Utc::now(),
        source: "smartd".into(),
        unit: None,
        severity: Severity::Info,
        message: "disk temperature normal".into(),
        raw: None,
        security: false,
    }).await;
    state.store = Arc::new(memory);
    state.chat_store = Arc::new(InMemoryChatStore::new(100));
    let app = router(Arc::new(state));
    let response = app
        .oneshot(axum::http::Request::get("/storage").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(body.contains("class=\"storage-grid\""), "storage grid missing");
    assert!(body.contains("storage-name") && body.contains("dev") && body.contains("sda1"), "disk name missing");
    assert!(body.contains("Recent Storage Events"), "storage events heading missing");
    assert!(body.contains("disk temperature normal"), "storage event row missing");
    assert!(body.contains("class=\"storage-sparkline\""), "storage sparkline missing");
    assert!(body.contains("id=\"storage-data\""), "storage data JSON missing");
}

#[tokio::test]
async fn network_page_renders_traffic_from_logs() {
    let mut state = AppState::new(Config::default(), Bus::new(), None, None).expect("state should build");
    let memory = InMemoryStore::new(10);
    memory.push_metric(sample_metric()).await;
    memory.push_log(LogEvent {
        ts: chrono::Utc::now(),
        source: "sshd".into(),
        unit: None,
        severity: Severity::Info,
        message: "Accepted publickey for user".into(),
        raw: None,
        security: false,
    }).await;
    state.store = Arc::new(memory);
    state.chat_store = Arc::new(InMemoryChatStore::new(100));
    let app = router(Arc::new(state));
    let response = app
        .oneshot(axum::http::Request::get("/network").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(body.contains("Recent Network Traffic"), "traffic heading missing");
    assert!(body.contains("Accepted publickey for user"), "traffic row missing");
    assert!(body.contains("class=\"network-sparkline\""), "sparkline missing");
}

#[tokio::test]
async fn events_stream_replies_with_sse_headers() {
    let state = test_state();
    let app = router(state);
    let response = app
        .oneshot(axum::http::Request::get("/events").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let headers = response.headers();
    assert_eq!(
        headers
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );
}

#[tokio::test]
async fn default_state_uses_fallback_when_no_api_key() {
    let state = AppState::new(Config::default(), Bus::new(), None, None).expect("state should build");
    assert!(state.llm.is_none(), "default config with no key should not create an LLM client");
    assert!(state.fallback.is_some(), "default config with no key should enable the fallback responder");
}

#[tokio::test]
async fn state_uses_fallback_for_empty_api_key() {
    let config = Config {
        llm_api_key: Some("".to_string()),
        ..Default::default()
    };
    let state = AppState::new(config, Bus::new(), None, None).expect("state should build");
    assert!(state.llm.is_none(), "empty API key should not create an LLM client");
    assert!(state.fallback.is_some(), "empty API key should enable the fallback responder");
}

#[tokio::test]
async fn state_uses_fallback_for_whitespace_api_key() {
    let config = Config {
        llm_api_key: Some("   ".to_string()),
        ..Default::default()
    };
    let state = AppState::new(config, Bus::new(), None, None).expect("state should build");
    assert!(state.llm.is_none(), "whitespace-only API key should not create an LLM client");
    assert!(state.fallback.is_some(), "whitespace-only API key should enable the fallback responder");
}

#[tokio::test]
async fn state_with_api_key_uses_llm_client() {
    let config = Config {
        llm_api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    let state = AppState::new(config, Bus::new(), None, None).expect("state should build");
    assert!(state.llm.is_some(), "config with an API key should create an LLM client");
    assert!(state.fallback.is_none(), "config with an API key should not enable the fallback responder");
}

#[tokio::test]
async fn chat_ask_html_with_fallback_responds_without_llm() {
    let _lock = CHAT_TEST_LOCK.lock().await;
    let state = AppState::new(Config::default(), Bus::new(), None, None).expect("state should build");
    state.clear_all_rate_limiters().await;
    assert!(state.fallback.is_some(), "default state should have fallback enabled");
    let app = router(Arc::new(state));

    let payload = json!({
        "session_id": Uuid::new_v4(),
        "owner_token": owner_token(),
        "message": "hello"
    })
    .to_string();
    let response = app
        .oneshot(
            axum::http::Request::post("/api/chat/ask-html")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(
        body.contains("Hello.") && body.contains("Observa"),
        "fallback greeting missing: {body}"
    );
}

#[tokio::test]
async fn metrics_partial_renders_summary_html() {
    let state = test_state_with_metric().await;
    let app = router(state);
    let response = app
        .oneshot(
            axum::http::Request::get("/partials/metrics-summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(body.contains("id=\"metrics-summary\""), "summary container missing");
    assert!(body.contains("CPU"), "cpu card missing");
    assert!(body.contains("Memory"), "memory card missing");
}

#[tokio::test]
async fn chat_ask_html_with_random_session_and_db_ensures_session() {
    let _lock = CHAT_TEST_LOCK.lock().await;
    let db = {
        let n = DB_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = format!("/tmp/observa_server_db_test_{n}.db");
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite://{path}");
        observa_db::Db::new(&url)
            .await
            .expect("test database should be created")
    };
    let mut state = AppState::new(Config::default(), Bus::new(), Some(db), None)
        .expect("state should build with test db");
    state.llm = None;
    state.fallback = Some(observa_llm::FallbackResponder::new());
    let state = Arc::new(state);
    state.clear_all_rate_limiters().await;
    let app = router(state.clone());

    let session_id = Uuid::new_v4();
    let payload = json!({
        "session_id": session_id,
        "owner_token": owner_token(),
        "message": "how is cpu?"
    })
    .to_string();
    let response = app
        .oneshot(
            axum::http::Request::post("/api/chat/ask-html")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    assert!(
        body.contains("CPU"),
        "fallback cpu answer missing: {body}"
    );

    let db = state.db.as_ref().expect("test db should be present");
    let stored = observa_db::chat::messages_for_session(db, session_id)
        .await
        .expect("messages should load without FK error");
    assert!(
        stored.iter().any(|m| m.role == observa_shared::Role::User && m.content == "how is cpu?"),
        "user message should be persisted"
    );
    assert!(
        stored.iter().any(|m| m.role == observa_shared::Role::Assistant),
        "assistant reply should be persisted"
    );
    assert!(
        !stored.iter().any(|m| m.role == observa_shared::Role::System),
        "system prompt should not be persisted"
    );
    assert!(
        !stored.iter().any(|m| m.content.starts_with("Latest metrics:")),
        "injected metric context should not be persisted"
    );
}

#[tokio::test]
async fn status_endpoint_renders_background_state() {
    let state = test_state();
    let app = router(state);
    let response = app
        .oneshot(axum::http::Request::get("/api/status").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body should be json");
    assert!(parsed.get("health").is_some(), "health missing");
    assert!(parsed.get("heartbeat_seq").is_some(), "heartbeat_seq missing");
    assert!(parsed.get("insight").is_some(), "insight missing");
}

#[tokio::test]
async fn insights_endpoint_renders_background_insight() {
    let state = test_state();
    let app = router(state);
    let response = app
        .oneshot(axum::http::Request::get("/api/insights").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body should be json");
    assert!(parsed.get("insight").is_some(), "insight missing");
}

#[tokio::test]
async fn health_endpoint_returns_json() {
    let state = test_state();
    let app = router(state);
    let response = app
        .oneshot(axum::http::Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body should be json");
    assert_eq!(parsed.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(parsed.get("status").and_then(|v| v.as_str()), Some("healthy"));
}

#[tokio::test]
async fn heartbeat_event_serializes_and_maps_to_sse_name() {
    let event = Event::Heartbeat(HeartbeatEvent {
        ts: chrono::Utc::now(),
        seq: 7,
    });
    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("\"Heartbeat\""), "variant name missing");
    assert!(json.contains("\"seq\":7"), "seq missing");
}

#[tokio::test]
async fn alert_event_serializes_and_maps_to_sse_name() {
    let event = Event::Alert(SecurityAlert {
        id: Uuid::new_v4(),
        ts: chrono::Utc::now(),
        source: "test".into(),
        unit: None,
        severity: Severity::Critical,
        message: "alert!".into(),
        raw: None,
        previous_hash: None,
        hash: "test-hash".into(),
    });
    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("\"Alert\""), "variant name missing");
    assert!(json.contains("alert!"), "message missing");
}

#[tokio::test]
async fn health_endpoint_reflects_degraded_state() {
    let state = test_state();
    state.background.set_health(HealthStatus::Degraded).await;
    let app = router(state);
    let response = app
        .oneshot(axum::http::Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body should be json");
    assert_eq!(parsed.get("ok").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(parsed.get("status").and_then(|v| v.as_str()), Some("degraded"));
}

#[tokio::test]
async fn health_endpoint_reflects_unhealthy_state() {
    let state = test_state();
    state.background.set_health(HealthStatus::Unhealthy).await;
    let app = router(state);
    let response = app
        .oneshot(axum::http::Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), 503);
    let body = body_string(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body should be json");
    assert_eq!(parsed.get("ok").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(parsed.get("status").and_then(|v| v.as_str()), Some("unhealthy"));
}

#[tokio::test]
async fn explain_log_without_llm_uses_fallback_responder() {
    let state = test_state();
    let app = router(state);
    let payload = serde_json::json!({
        "log": {
            "ts": chrono::Utc::now(),
            "source": "auth",
            "severity": "Error",
            "message": "Failed password for admin from 192.168.1.42"
        }
    })
    .to_string();
    let response = app
        .oneshot(
            axum::http::Request::post("/api/logs/explain")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body should be json");
    let explanation = parsed
        .get("explanation")
        .and_then(|v| v.as_str())
        .expect("explanation should be present");
    assert!(
        explanation.contains("Observa") || explanation.contains("password") || explanation.contains("log"),
        "fallback explanation missing expected content: {explanation}"
    );
}

#[tokio::test]
async fn explain_log_without_llm_nor_fallback_returns_unprocessable_entity() {
    let app = router(state_without_fallback());
    let payload = serde_json::json!({
        "log": {
            "ts": chrono::Utc::now(),
            "source": "test",
            "severity": "Info",
            "message": "unit test"
        }
    })
    .to_string();
    let response = app
        .oneshot(
            axum::http::Request::post("/api/logs/explain")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn explain_log_with_mock_llm_returns_reply() {
    let config = Config {
        llm_api_base: mock_llm_url().await,
        llm_api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    let state = Arc::new(
        AppState::new(config, Bus::new(), None, None).expect("state should build with mock llm"),
    );
    let app = router(state);
    let payload = serde_json::json!({
        "log": {
            "ts": chrono::Utc::now(),
            "source": "kernel",
            "severity": "Warn",
            "message": "CPU temperature high"
        }
    })
    .to_string();
    let response = app
        .oneshot(
            axum::http::Request::post("/api/logs/explain")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = body_string(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("body should be json");
    let explanation = parsed
        .get("explanation")
        .and_then(|v| v.as_str())
        .expect("explanation should be present");
    assert!(explanation.contains("Echo:"), "mock LLM should echo system prompt: {explanation}");
}

#[tokio::test]
async fn nonexistent_page_returns_404() {
    let state = test_state();
    let app = router(state);
    let response = app
        .oneshot(
            axum::http::Request::get("/definitely-not-a-page")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn chat_ask_rate_limit_returns_429_after_threshold() {
    let _lock = CHAT_TEST_LOCK.lock().await;
    let state = state_without_fallback();
    state.clear_all_rate_limiters().await;
    let app = router(state);
    let payload = serde_json::json!({
        "session_id": Uuid::new_v4(),
        "owner_token": owner_token(),
        "message": "hi"
    })
    .to_string();
    for i in 0..25 {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::post("/api/chat/ask")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        if i < 20 {
            assert_eq!(response.status(), 422, "request {} should be 422 (no LLM), got {}", i, response.status());
        } else {
            assert_eq!(response.status(), 429, "request {} should be 429 (rate limited), got {}", i, response.status());
        }
    }
}

#[tokio::test]
async fn acknowledge_alert_rate_limit_returns_429_after_threshold() {
    let _lock = CHAT_TEST_LOCK.lock().await;
    let state = test_state();
    state.clear_all_rate_limiters().await;
    let app = router(state);
    let payload = serde_json::json!({ "key": "test-alert" }).to_string();
    for i in 0..35 {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::post("/api/alerts/acknowledge")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        if i < 30 {
            assert_eq!(response.status(), 200, "request {} should be 200, got {}", i, response.status());
        } else {
            assert_eq!(response.status(), 429, "request {} should be 429 (rate limited), got {}", i, response.status());
        }
    }
}

#[tokio::test]
async fn misleading_llm_summary_keeps_data_driven_healthy_classification() {
    let config = Config {
        llm_api_base: mock_alarmist_llm_url().await,
        llm_api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    let state = Arc::new(
        AppState::new(config, Bus::new(), None, None).expect("state should build with mock llm"),
    );

    let metrics = vec![sample_metric()];
    let logs = vec![LogEvent {
        ts: chrono::Utc::now(),
        source: "test".to_string(),
        unit: None,
        severity: Severity::Info,
        message: "routine info log".to_string(),
        raw: None,
        security: false,
    }];

    let summary = observa_server::insight::generate(&state, &metrics, &logs)
        .await
        .expect("insight generation should succeed");
    let lower = summary.to_lowercase();
    assert!(
        lower.contains("critical") || lower.contains("fire") || lower.contains("unhealthy"),
        "mock LLM should return an alarmist summary, got: {summary}"
    );

    let latest = metrics.last().expect("metrics non-empty");
    let cpu = latest.cpu.usage_percent;
    let memory_pct = if latest.memory.total_bytes == 0 {
        0.0
    } else {
        100.0 * latest.memory.used_bytes as f64 / latest.memory.total_bytes as f64
    };

    let health = observa_server::background::health_from_data(&logs, cpu, memory_pct, false);
    let severity = observa_server::background::alert_severity_from_data(&logs, cpu, memory_pct, false);

    assert_eq!(
        health,
        HealthStatus::Healthy,
        "alarmist wording should not make a healthy system appear degraded; summary: {summary}"
    );
    assert_eq!(
        severity,
        Severity::Info,
        "quiet logs and moderate resources should not raise a security alert; summary: {summary}"
    );
}
