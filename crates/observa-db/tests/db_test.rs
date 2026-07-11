use std::sync::atomic::{AtomicU64, Ordering};

use observa_db::{chat, logs, metrics, Db};
use observa_shared::{
    AiServerKind, AiServerMetrics, ChatMessage, CpuMetrics, DiskMetrics, LogEvent, MemoryMetrics,
    MetricSnapshot, NetworkMetrics, ProcessMetrics, Role, Severity, SwapMetrics,
};

static DB_COUNTER: AtomicU64 = AtomicU64::new(0);

async fn temp_db() -> Db {
    let n = DB_COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = format!("/tmp/observa_db_test_{n}.db");
    let _ = std::fs::remove_file(&path);
    let url = format!("sqlite://{path}");
    Db::new(&url).await.expect("failed to create db")
}

fn sample_snapshot() -> MetricSnapshot {
    MetricSnapshot {
        ts: chrono::Utc::now(),
        cpu: CpuMetrics {
            usage_percent: 12.5,
            per_core_usage: vec![10.0, 15.0],
            frequency_mhz: 2400,
        },
        memory: MemoryMetrics {
            total_bytes: 16_000_000_000,
            used_bytes: 8_000_000_000,
            free_bytes: 8_000_000_000,
        },
        disks: vec![DiskMetrics {
            name: "root".into(),
            total_bytes: 500_000_000_000,
            used_bytes: 100_000_000_000,
            read_bytes_per_sec: 1_000_000.0,
            write_bytes_per_sec: 500_000.0,
        }],
        networks: vec![NetworkMetrics {
            interface: "eth0".to_string(),
            rx_bytes: 1000,
            tx_bytes: 2000,
            rx_rate: 0.0,
            tx_rate: 0.0,
        }],
        processes: vec![ProcessMetrics {
            pid: 1,
            name: "init".into(),
            cmdline: None,
            cpu_percent: 0.1,
            memory_bytes: 10_000_000,
        }],
        gpu: Vec::new(),
        swap: Some(SwapMetrics {
            total_bytes: 4_000_000_000,
            used_bytes: 500_000_000,
            free_bytes: 3_500_000_000,
        }),
        ai_servers: vec![AiServerMetrics {
            pid: 1234,
            kind: AiServerKind::Ollama,
            name: "ollama".into(),
            port_hint: None,
            endpoint: None,
            models: Vec::new(),
            cpu_percent: 5.0,
            memory_bytes: 100_000_000,
        }],
    }
}

#[tokio::test]
async fn metrics_roundtrip() {
    let db = temp_db().await;
    let snapshot = sample_snapshot();

    metrics::store(&db, &snapshot, true)
        .await
        .expect("store metric failed");

    let recent = metrics::recent(&db, 10)
        .await
        .expect("recent metrics failed");

    assert_eq!(recent.len(), 1);
    assert!((recent[0].cpu.usage_percent - snapshot.cpu.usage_percent).abs() < f32::EPSILON);
}

#[tokio::test]
async fn logs_roundtrip() {
    let db = temp_db().await;
    let log = LogEvent {
        ts: chrono::Utc::now(),
        source: "journalctl".into(),
        unit: Some("nginx.service".into()),
        severity: Severity::Error,
        message: "connection refused".into(),
        raw: Some(serde_json::json!({"_HOSTNAME": "foo"})),
        security: false,
    };

    logs::store(&db, &log).await.expect("store log failed");
    let recent = logs::recent(&db, 10).await.expect("recent logs failed");

    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].severity, Severity::Error);
    assert_eq!(recent[0].message, "connection refused");
    assert!(recent[0].raw.is_some());
}

#[tokio::test]
async fn chat_roundtrip() {
    let db = temp_db().await;
    let (session_id, owner_token) = chat::create_session(&db)
        .await
        .expect("create session failed");

    let messages = vec![
        ChatMessage {
            role: Role::User,
            content: "hello".into(),
        },
        ChatMessage {
            role: Role::Assistant,
            content: "hi".into(),
        },
    ];

    for msg in &messages {
        chat::store_message(&db, session_id, msg)
            .await
            .expect("store message failed");
    }

    let stored = chat::messages_for_session(&db, session_id)
        .await
        .expect("read messages failed");
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].role, Role::User);
    assert_eq!(stored[1].content, "hi");

    assert!(
        chat::verify_session_owner(&db, session_id, &owner_token)
            .await
            .expect("verify owner failed")
    );
    assert!(
        !chat::verify_session_owner(&db, session_id, "wrong-token")
            .await
            .expect("verify owner failed")
    );
}

#[tokio::test]
async fn search_logs_filters_by_message_and_severity() {
    let db = temp_db().await;
    let events = vec![
        LogEvent {
            ts: chrono::Utc::now(),
            source: "journald".into(),
            unit: None,
            severity: Severity::Error,
            message: "disk full on /dev/sda1".into(),
            raw: None,
            security: false,
        },
        LogEvent {
            ts: chrono::Utc::now(),
            source: "journald".into(),
            unit: None,
            severity: Severity::Info,
            message: "started nginx service".into(),
            raw: None,
            security: false,
        },
        LogEvent {
            ts: chrono::Utc::now(),
            source: "journald".into(),
            unit: None,
            severity: Severity::Error,
            message: "connection refused".into(),
            raw: None,
            security: false,
        },
    ];

    for evt in &events {
        logs::store(&db, evt).await.expect("store log failed");
    }

    let results = logs::search(
        &db,
        Some("disk"),
        &[Severity::Error, Severity::Critical],
        10,
    )
    .await
    .expect("search logs failed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].message, "disk full on /dev/sda1");

    let error_only = logs::search(&db, None, &[Severity::Error], 10)
        .await
        .expect("severity search failed");
    assert_eq!(error_only.len(), 2);

    let no_match = logs::search(&db, Some("nginx"), &[Severity::Error], 10)
        .await
        .expect("search logs failed");
    assert!(no_match.is_empty());
}

#[tokio::test]
async fn compressed_metric_roundtrips() {
    let db = temp_db().await;
    let snapshot = sample_snapshot();

    metrics::store(&db, &snapshot, true)
        .await
        .expect("store compressed metric failed");

    let recent = metrics::recent(&db, 10).await.expect("recent metrics failed");
    assert_eq!(recent.len(), 1);
    assert!((recent[0].cpu.usage_percent - snapshot.cpu.usage_percent).abs() < f32::EPSILON);
}

#[tokio::test]
async fn uncompressed_metric_roundtrips() {
    let db = temp_db().await;
    let snapshot = sample_snapshot();

    metrics::store(&db, &snapshot, false)
        .await
        .expect("store uncompressed metric failed");

    let recent = metrics::recent(&db, 10).await.expect("recent metrics failed");
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].memory.total_bytes, snapshot.memory.total_bytes);
}

#[tokio::test]
async fn prune_metrics_older_than_retention_days() {
    let db = temp_db().await;
    let mut old = sample_snapshot();
    old.ts = chrono::Utc::now() - chrono::Duration::days(10);
    let mut recent = sample_snapshot();
    recent.ts = chrono::Utc::now() - chrono::Duration::days(1);

    metrics::store(&db, &old, true).await.expect("store old metric");
    metrics::store(&db, &recent, true)
        .await
        .expect("store recent metric");

    assert_eq!(metrics::row_count(&db).await.expect("row count"), 2);

    let pruned = db.prune_metrics(7).await.expect("prune metrics");
    assert_eq!(pruned, 1);
    assert_eq!(metrics::row_count(&db).await.expect("row count after prune"), 1);

    let remaining = metrics::recent(&db, 10).await.expect("recent after prune");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].ts, recent.ts);
}

#[tokio::test]
async fn prune_logs_older_than_retention_days() {
    let db = temp_db().await;
    let old = LogEvent {
        ts: chrono::Utc::now() - chrono::Duration::days(10),
        source: "journald".into(),
        unit: None,
        severity: Severity::Error,
        message: "old error".into(),
        raw: None,
        security: false,
    };
    let recent = LogEvent {
        ts: chrono::Utc::now() - chrono::Duration::days(1),
        source: "journald".into(),
        unit: None,
        severity: Severity::Info,
        message: "recent info".into(),
        raw: None,
        security: false,
    };

    logs::store(&db, &old).await.expect("store old log");
    logs::store(&db, &recent).await.expect("store recent log");

    assert_eq!(logs::row_count(&db).await.expect("log row count"), 2);

    let pruned = db.prune_logs(7).await.expect("prune logs");
    assert_eq!(pruned, 1);
    assert_eq!(logs::row_count(&db).await.expect("log row count after prune"), 1);

    let remaining = logs::recent(&db, 10).await.expect("recent logs after prune");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].message, "recent info");
}

#[tokio::test]
async fn vacuum_reclaims_space_after_prune() {
    let db = temp_db().await;
    for i in 0..20 {
        let mut s = sample_snapshot();
        s.ts = chrono::Utc::now() - chrono::Duration::hours(i);
        metrics::store(&db, &s, true).await.expect("store metric");
    }

    let before = db.prune_metrics(0).await.expect("prune all metrics");
    assert_eq!(before, 20);

    db.vacuum().await.expect("vacuum database");

    assert_eq!(metrics::row_count(&db).await.expect("count after vacuum"), 0);
}
