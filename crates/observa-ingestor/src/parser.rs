use chrono::{DateTime, TimeZone, Utc};
use observa_shared::{LogEvent, ObservaError, Result, Severity};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct JournalEntry {
    #[serde(rename = "__REALTIME_TIMESTAMP")]
    realtime_timestamp: Option<String>,
    #[serde(rename = "_SYSTEMD_UNIT")]
    unit: Option<String>,
    #[serde(rename = "PRIORITY")]
    priority: Option<String>,
    #[serde(rename = "MESSAGE")]
    message: Option<String>,
}



/// Convert a journalctl `--output=json` line into a `LogEvent`.
///
/// Returns `Ok(None)` when the line is empty, and `Err` when JSON parsing fails.
pub fn parse_journalctl_json(line: &str) -> Result<Option<LogEvent>> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }

    let raw: Value = serde_json::from_str(line)
        .map_err(|e| ObservaError::Config(format!("invalid journalctl JSON: {e}")))?;
    let entry: JournalEntry = serde_json::from_value(raw.clone())
        .map_err(|e| ObservaError::Config(format!("unexpected journalctl entry: {e}")))?;

    let ts = entry
        .realtime_timestamp
        .as_deref()
        .and_then(parse_journal_timestamp)
        .unwrap_or_else(Utc::now);

    let message = entry.message.unwrap_or_default();
    let severity = entry
        .priority
        .as_deref()
        .and_then(map_journal_priority)
        .unwrap_or(Severity::Info);

    let security = is_security_event(&message, entry.unit.as_deref().unwrap_or(""), "journald");

    Ok(Some(LogEvent {
        ts,
        source: "journald".to_string(),
        unit: entry.unit,
        severity,
        message,
        raw: Some(raw),
        security,
    }))
}

fn parse_journal_timestamp(value: &str) -> Option<DateTime<Utc>> {
    // journalctl emits microseconds since epoch.
    let micros: i64 = value.parse().ok()?;
    Utc.timestamp_micros(micros).single()
}

fn map_journal_priority(value: &str) -> Option<Severity> {
    match value {
        "0" | "1" | "2" => Some(Severity::Critical),
        "3" => Some(Severity::Error),
        "4" => Some(Severity::Warn),
        "5" | "6" => Some(Severity::Info),
        "7" => Some(Severity::Debug),
        _ => None,
    }
}

/// Convert a plain log line into a `LogEvent`.
///
/// No structured parsing is attempted; the whole line becomes the message and
/// the severity defaults to `Info`.
pub fn parse_fallback_line(line: &str) -> Result<LogEvent> {
    let message = line.trim().to_string();
    let security = is_security_event(&message, "", "file");
    Ok(LogEvent {
        ts: Utc::now(),
        source: "file".to_string(),
        unit: None,
        severity: Severity::Info,
        message,
        raw: None,
        security,
    })
}

const SECURITY_PATTERNS: [&str; 12] = [
    "failed password",
    "authentication failure",
    "invalid user",
    "sudo:",
    "connection closed by",
    "refused",
    "denied",
    "unauthorized",
    "attack",
    "brute",
    "firewall",
    "iptables",
];

fn is_security_event(message: &str, unit: &str, source: &str) -> bool {
    let text = message.to_lowercase();
    let source_hits = source == "journald"
        && (unit.to_lowercase().contains("ssh")
            || unit.to_lowercase().contains("auth")
            || unit.to_lowercase().contains("sudo"));
    let message_hits = SECURITY_PATTERNS.iter().any(|pat| text.contains(pat));
    source_hits || message_hits
}
