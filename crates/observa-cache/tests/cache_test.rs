use observa_cache::Cache;
use observa_shared::{
    AiServerKind, AiServerMetrics, CpuMetrics, DiskMetrics, LogEvent, MemoryMetrics, MetricSnapshot,
    NetworkMetrics, ProcessMetrics, Severity, SwapMetrics,
};

fn sample_metric() -> MetricSnapshot {
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
            interface: "eth0".into(),
            rx_bytes: 1_000,
            tx_bytes: 2_000,
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

fn sample_log() -> LogEvent {
    LogEvent {
        ts: chrono::Utc::now(),
        source: "journalctl".into(),
        unit: Some("nginx.service".into()),
        severity: Severity::Error,
        message: "connection refused".into(),
        raw: Some(serde_json::json!({"_HOSTNAME": "foo"})),
        security: false,
    }
}

#[tokio::test]
async fn none_url_is_degraded() {
    let cache = Cache::new(None)
        .await
        .expect("cache should build without redis");
    assert!(!cache.is_available());
}

#[tokio::test]
async fn metrics_roundtrip_in_fallback() {
    let cache = Cache::new(None).await.unwrap();
    let metric = sample_metric();

    cache
        .push_recent_metric(&metric)
        .await
        .expect("push failed");
    let recent = cache.recent_metrics(10).await.expect("recent failed");

    assert_eq!(recent.len(), 1);
    assert!((recent[0].cpu.usage_percent - metric.cpu.usage_percent).abs() < f32::EPSILON);
}

#[tokio::test]
async fn logs_roundtrip_in_fallback() {
    let cache = Cache::new(None).await.unwrap();
    let log = sample_log();

    cache.push_recent_log(&log).await.expect("push failed");
    let recent = cache.recent_logs(10).await.expect("recent failed");

    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].severity, Severity::Error);
    assert_eq!(recent[0].message, "connection refused");
}
