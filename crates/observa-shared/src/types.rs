use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogSource {
    Journald,
    File { path: std::path::PathBuf },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CpuMetrics {
    pub usage_percent: f32,
    pub per_core_usage: Vec<f32>,
    pub frequency_mhz: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwapMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskMetrics {
    pub name: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    #[serde(default)]
    pub read_bytes_per_sec: f32,
    #[serde(default)]
    pub write_bytes_per_sec: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkMetrics {
    pub interface: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_rate: f32,
    pub tx_rate: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiServerKind {
    Vllm,
    Ollama,
    Triton,
    OpenAi,
    Sglang,
    LlamaCpp,
    ExllamaV2,
    KoboldCpp,
    TabbyApi,
    LmStudio,
    TextGenerationInference,
    #[default]
    Generic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiServerMetrics {
    pub pid: u32,
    pub kind: AiServerKind,
    pub name: String,
    pub port_hint: Option<u16>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessMetrics {
    pub pid: u32,
    pub name: String,
    #[serde(default)]
    pub cmdline: Option<String>,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuMetrics {
    pub name: String,
    pub usage_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub bandwidth_bytes_per_sec: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricSnapshot {
    pub ts: DateTime<Utc>,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    #[serde(default)]
    pub swap: Option<SwapMetrics>,
    pub disks: Vec<DiskMetrics>,
    pub networks: Vec<NetworkMetrics>,
    pub processes: Vec<ProcessMetrics>,
    #[serde(default)]
    pub gpu: Vec<GpuMetrics>,
    #[serde(default)]
    pub ai_servers: Vec<AiServerMetrics>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEvent {
    pub ts: DateTime<Utc>,
    pub source: String,
    pub unit: Option<String>,
    pub severity: Severity,
    pub message: String,
    pub raw: Option<serde_json::Value>,
    #[serde(default)]
    pub security: bool,
}

/// An append-only, tamper-evident security alert.
///
/// `previous_hash` links this record to the previous alert; `hash` covers the
/// alert content plus the previous hash.  The chain can be verified by any
/// reader that recomputes the hashes in timestamp order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecurityAlert {
    pub id: Uuid,
    pub ts: DateTime<Utc>,
    pub source: String,
    pub unit: Option<String>,
    pub severity: Severity,
    pub message: String,
    pub raw: Option<serde_json::Value>,
    pub previous_hash: Option<String>,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatSessionSummary {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub last_message_preview: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatEvent {
    pub ts: DateTime<Utc>,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsightSnapshot {
    pub ts: DateTime<Utc>,
    pub summary: String,
    pub health: HealthStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPreferences {
    pub theme: String,
    pub refresh_interval_ms: u64,
    pub reduced_motion: bool,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            theme: String::from("dark"),
            refresh_interval_ms: 2000,
            reduced_motion: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Metric(MetricSnapshot),
    Log(LogEvent),
    Chat(ChatMessage),
    Heartbeat(HeartbeatEvent),
    Alert(SecurityAlert),
}

pub fn format_bytes(n: u64) -> String {
    const UNITS: &[u8] = b"BKMGTPE";
    if n == 0 {
        return "0 B".to_string();
    }
    let exp = ((n as f64).log10() / 1024f64.log10()).min((UNITS.len() - 1) as f64) as usize;
    let scaled = n as f64 / 1024f64.powi(exp as i32);
    format!("{scaled:.1} {}", UNITS[exp] as char)
}

/// Format a network throughput rate as a human-readable string.
pub fn format_rate(bytes_per_sec: f32) -> String {
    if bytes_per_sec >= 1_000_000_000.0 {
        format!("{:.2} GB/s", bytes_per_sec / 1_000_000_000.0)
    } else if bytes_per_sec >= 1_000_000.0 {
        format!("{:.2} MB/s", bytes_per_sec / 1_000_000.0)
    } else if bytes_per_sec >= 1_000.0 {
        format!("{:.1} KB/s", bytes_per_sec / 1_000.0)
    } else {
        format!("{:.0} B/s", bytes_per_sec)
    }
}

/// Render a memory percentage from used and total bytes.
pub fn memory_pct(used: u64, total: u64) -> String {
    if total == 0 {
        return "0%".to_string();
    }
    format!("{:.0}%", 100.0 * used as f64 / total as f64)
}

/// Map a severity to a CSS class name for rendering.
pub fn severity_class(s: Severity) -> &'static str {
    match s {
        Severity::Debug => "debug",
        Severity::Info => "info",
        Severity::Warn => "warn",
        Severity::Error => "error",
        Severity::Critical => "critical",
    }
}

impl Event {
    pub const fn is_metric(&self) -> bool {
        matches!(self, Event::Metric(_))
    }

    pub const fn is_log(&self) -> bool {
        matches!(self, Event::Log(_))
    }

    pub const fn is_chat(&self) -> bool {
        matches!(self, Event::Chat(_))
    }

    pub const fn is_heartbeat(&self) -> bool {
        matches!(self, Event::Heartbeat(_))
    }

    pub const fn is_alert(&self) -> bool {
        matches!(self, Event::Alert(_))
    }
}
