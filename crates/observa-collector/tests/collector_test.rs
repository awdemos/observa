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

    // The bus keeps a `_receiver` alive so the channel never closes; wait for a
    // Metric event with a generous timeout because `normalize()` can be slow.
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match rx.recv().await {
                Ok(observa_shared::Event::Metric(_)) => break Ok(()),
                Ok(_) => continue,
                Err(_) => break Err("bus closed"),
            }
        }
    })
    .await;
    drop(handle);

    assert!(event.is_ok(), "timed out waiting for metric event");
    assert!(event.unwrap().is_ok(), "expected Metric event but bus closed");
}
