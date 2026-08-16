use std::sync::Arc;
use std::time::Duration;

use axum::{routing::get, Json, Router};
use observa_bus::Bus;
use observa_config::{AiServerConfig, Config};
use observa_server::ai_poll::{spawn_ai_server_poller, AiServerCache};
use observa_shared::AiServerStatus;
use serde_json::json;
use tokio::sync::{watch, RwLock};

fn test_config(endpoint: String) -> Config {
    Config {
        database_url: None,
        redis_url: None,
        bind_addr: "127.0.0.1:0".to_string(),
        llm_api_base: "http://localhost:8080/v1".to_string(),
        llm_model: "test".to_string(),
        llm_api_key: None,
        llm_timeout_secs: 120,
        sample_interval_ms: 2000,
        log_source: observa_shared::LogSource::Journald,
        log_tail: true,
        retention_days: 7,
        compression_enabled: false,
        vacuum_interval_hours: 24,
        notifications_enabled: false,
        notifications_webhook_url: None,
        log_page_size: 50,
        metric_history_minutes: 60,
        ai_servers: vec![AiServerConfig {
            endpoint,
            name: Some("mock-ollama".to_string()),
            kind: Some("ollama".to_string()),
        }],
        ai_server_poll_interval_ms: 100,
        ai_server_probe_timeout_ms: 1_000,
        ai_server_subnet_scan: false,
        dashboard_token: None,
    }
}

async fn mock_server() -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new().route(
        "/v1/models",
        get(|| async {
            Json(json!({
                "object": "list",
                "data": [
                    {"id": "mock-model", "object": "model"}
                ]
            }))
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();
    });
    (format!("http://{}/v1", addr), handle)
}

#[tokio::test]
async fn poller_marks_remote_endpoint_online_and_lists_models() {
    let (endpoint, _server) = mock_server().await;
    let config = Arc::new(test_config(endpoint));
    let cache: AiServerCache = Arc::new(RwLock::new(Vec::new()));
    let bus = Bus::new();
    let (_tx, shutdown) = watch::channel(false);

    let handle = spawn_ai_server_poller(config, cache.clone(), bus, shutdown);

    // Wait for the initial probe.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let servers = cache.read().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].status, AiServerStatus::Online);
    assert_eq!(servers[0].models, vec!["mock-model"]);
    assert!(servers[0].latency_ms.is_some());

    handle.abort();
}

#[tokio::test]
async fn poller_marks_unreachable_endpoint_offline() {
    let config = Arc::new(test_config("http://127.0.0.1:1/v1".to_string()));
    let cache: AiServerCache = Arc::new(RwLock::new(Vec::new()));
    let bus = Bus::new();
    let (_tx, shutdown) = watch::channel(false);

    let handle = spawn_ai_server_poller(config, cache.clone(), bus, shutdown);

    tokio::time::sleep(Duration::from_millis(300)).await;

    let servers = cache.read().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].status, AiServerStatus::Offline);
    assert!(servers[0].last_error.is_some());

    handle.abort();
}

fn exo_test_config(endpoint: String) -> Config {
    Config {
        ai_servers: vec![AiServerConfig {
            endpoint,
            name: Some("mock-exo".to_string()),
            kind: Some("exo".to_string()),
        }],
        ..test_config(String::new())
    }
}

async fn mock_exo_server() -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [
                        {"id": "llama-3.2-1b", "object": "model"}
                    ]
                }))
            }),
        )
        .route(
            "/state",
            get(|| async {
                Json(json!({
                    "topology": {
                        "nodes": {
                            "node-a": {},
                            "node-b": {}
                        }
                    }
                }))
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();
    });
    (format!("http://{}/v1", addr), handle)
}

#[tokio::test]
async fn poller_detects_exo_cluster_and_extracts_nodes() {
    let (endpoint, _server) = mock_exo_server().await;
    let config = Arc::new(exo_test_config(endpoint));
    let cache: AiServerCache = Arc::new(RwLock::new(Vec::new()));
    let bus = Bus::new();
    let (_tx, shutdown) = watch::channel(false);

    let handle = spawn_ai_server_poller(config, cache.clone(), bus, shutdown);

    tokio::time::sleep(Duration::from_millis(300)).await;

    let servers = cache.read().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].status, AiServerStatus::Online);
    assert_eq!(servers[0].models, vec!["llama-3.2-1b"]);
    assert_eq!(servers[0].kind, observa_shared::AiServerKind::Exo);
    let nodes = servers[0].cluster_nodes.as_ref().expect("cluster nodes expected");
    assert_eq!(nodes.len(), 2);
    assert!(nodes.contains(&"node-a".to_string()));
    assert!(nodes.contains(&"node-b".to_string()));

    handle.abort();
}
