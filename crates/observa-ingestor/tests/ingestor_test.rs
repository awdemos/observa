use observa_ingestor::{parse_fallback_line, parse_journalctl_json};
use observa_shared::{Event, Severity};

#[test]
fn parse_journalctl_json_produces_log_event() {
    let line = r#"{"__REALTIME_TIMESTAMP":"1699000000000000","_SYSTEMD_UNIT":"test.service","PRIORITY":"3","MESSAGE":"disk full"}"#;
    let event = parse_journalctl_json(line)
        .expect("failed to parse journalctl line")
        .expect("empty journalctl line");

    assert_eq!(event.source, "journald");
    assert_eq!(event.unit.as_deref(), Some("test.service"));
    assert_eq!(event.severity, Severity::Error);
    assert_eq!(event.message, "disk full");
}

#[test]
fn parse_fallback_line_maps_unknown_severity_to_info() {
    let line = "2026-07-07T12:34:56Z app[123]: hello world";
    let event = parse_fallback_line(line).expect("failed to parse fallback line");

    assert_eq!(event.source, "file");
    assert_eq!(event.severity, Severity::Info);
    assert!(event.message.contains("hello world"));
}

#[tokio::test]
async fn ingestor_publishes_fallback_lines_from_file() {
    use std::io::Write;
    use tokio::time::timeout;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.log");
    {
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "line one").unwrap();
    }

    let bus = observa_bus::Bus::new();
    let mut rx = bus.subscribe();
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let _handle = observa_ingestor::spawn_ingestor(observa_ingestor::IngestorOpts {
        source: observa_shared::LogSource::File { path: path.clone() },
        tail: false,
        db: None,
        cache: None,
        bus,
        shutdown: shutdown_rx,
    });

    let event = timeout(std::time::Duration::from_secs(2), rx.recv()).await;
    let _ = shutdown_tx.send(true);

    assert!(event.is_ok(), "timed out waiting for log event");
    assert!(
        matches!(event.unwrap(), Ok(Event::Log(_))),
        "expected Log event"
    );
}
