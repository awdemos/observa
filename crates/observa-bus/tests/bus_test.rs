use std::time::Duration;

use chrono::Utc;
use observa_bus::Bus;
use observa_shared::{
    AiServerKind, AiServerMetrics, ChatMessage, CpuMetrics, DiskMetrics, Event, LogEvent,
    MemoryMetrics, MetricSnapshot, NetworkMetrics, ProcessMetrics, Role, Severity, SwapMetrics,
};
use tokio::time::timeout;
use tokio_stream::StreamExt;

fn metric_event() -> Event {
    Event::Metric(MetricSnapshot {
        ts: Utc::now(),
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
            name: "sda".to_string(),
            total_bytes: 500_000_000_000,
            used_bytes: 100_000_000_000,
            read_bytes_per_sec: 1_000_000.0,
            write_bytes_per_sec: 500_000.0,
        }],
        networks: vec![NetworkMetrics {
            interface: "eth0".to_string(),
            rx_bytes: 1_000,
            tx_bytes: 2_000,
            rx_rate: 0.0,
            tx_rate: 0.0,
        }],
        processes: vec![ProcessMetrics {
            pid: 42,
            name: "observa".to_string(),
            cmdline: None,
            cpu_percent: 1.2,
            memory_bytes: 50_000_000,
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
    })
}

fn log_event() -> Event {
    Event::Log(LogEvent {
        ts: Utc::now(),
        source: "test".to_string(),
        unit: None,
        severity: Severity::Info,
        message: "hello bus".to_string(),
        raw: None,
        security: false,
    })
}

fn chat_event() -> Event {
    Event::Chat(ChatMessage {
        role: Role::User,
        content: "ping".to_string(),
    })
}

#[tokio::test]
async fn publish_and_subscribe() {
    let bus = Bus::new();
    let mut rx = bus.subscribe();

    let event = metric_event();
    bus.publish(event.clone()).unwrap();

    let received = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("timed out waiting for event")
        .expect("channel closed");

    assert_eq!(received, event);
}

#[tokio::test]
async fn multiple_subscribers_receive_event() {
    let bus = Bus::new();
    let mut first = bus.subscribe();
    let mut second = bus.subscribe();

    let event = log_event();
    bus.publish(event.clone()).unwrap();

    let a = timeout(Duration::from_secs(1), first.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    let b = timeout(Duration::from_secs(1), second.recv())
        .await
        .expect("timed out")
        .expect("channel closed");

    assert_eq!(a, event);
    assert_eq!(b, event);
}

#[tokio::test]
async fn event_stream_maps_broadcast_messages() {
    let bus = Bus::new();
    let mut stream = observa_bus::event_stream(&bus);

    let event = chat_event();
    bus.publish(event.clone()).unwrap();

    let received = timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("timed out")
        .expect("stream ended early");

    assert_eq!(received, event);
}
