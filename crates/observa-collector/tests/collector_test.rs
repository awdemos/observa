use observa_collector::normalize;
use sysinfo::System;

#[test]
fn normalize_returns_snapshot() {
    let mut system = System::new_all();
    system.refresh_all();

    let snapshot = normalize(&system);

    assert!(!snapshot.cpu.per_core_usage.is_empty(), "cpu usage missing");
    assert!(snapshot.memory.total_bytes > 0, "memory total missing");
    assert!(snapshot.memory.used_bytes <= snapshot.memory.total_bytes);
}

#[tokio::test]
async fn collector_publishes_metric_event() {
    let bus = observa_bus::Bus::new();
    let mut rx = bus.subscribe();

    let (_tx, shutdown) = tokio::sync::watch::channel(false);
    let handle = observa_collector::spawn_collector(observa_collector::CollectorOpts {
        interval_ms: 50,
        db: None,
        cache: None,
        bus: bus.clone(),
        shutdown,
        compression_enabled: true,
        ai_server_endpoints: Vec::new(),
        ai_server_subnet_scan: false,
    });

    // Drop handle after receiving at least one metric event.
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await;
    drop(handle);

    assert!(event.is_ok(), "timed out waiting for metric event");
    assert!(
        matches!(event.unwrap(), Ok(observa_shared::Event::Metric(_))),
        "expected Metric event"
    );
}
