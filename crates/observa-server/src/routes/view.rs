use observa_shared::{
    format_bytes, format_rate, severity_class, AiServerMetrics, DiskMetrics, GpuMetrics, LogEvent,
    MetricSnapshot, NetworkMetrics, ProcessMetrics, SecurityAlert,
};

pub fn storage_sparkline(read_rate: f32, write_rate: f32) -> Vec<f32> {
    const SAMPLES: usize = 12;
    const MAX_RATE: f32 = 200_000_000.0; // 200 MB/s reference
    let total = read_rate + write_rate;
    let baseline = (total / MAX_RATE * 100.0).clamp(0.0, 100.0);
    (0..SAMPLES)
        .map(|i| {
            let phase = (i as f32 * 0.7 + baseline).sin().abs();
            let value = baseline * (0.6 + 0.4 * phase);
            value.clamp(0.0, 100.0)
        })
        .collect()
}

pub fn network_sparkline(rx_rate: f32, tx_rate: f32) -> Vec<f32> {
    const SAMPLES: usize = 12;
    const MAX_RATE: f32 = 100_000_000.0;
    let total = rx_rate + tx_rate;
    let baseline = (total / MAX_RATE * 100.0).clamp(0.0, 100.0);
    (0..SAMPLES)
        .map(|i| {
            // deterministic synthetic wiggle so every card isn't identical
            let phase = (i as f32 + baseline).sin().abs();
            let value = baseline * (0.6 + 0.4 * phase);
            value.clamp(0.0, 100.0)
        })
        .collect()
}

pub fn process_sparkline(cpu_pct: f32, memory_bytes: f32) -> Vec<f32> {
    const SAMPLES: usize = 12;
    let baseline = (cpu_pct + (memory_bytes / 100_000_000.0)).clamp(0.0, 100.0);
    (0..SAMPLES)
        .map(|i| {
            let phase = (i as f32 * 0.5 + baseline * 0.1).sin().abs();
            let value = baseline * (0.6 + 0.4 * phase);
            value.clamp(0.0, 100.0)
        })
        .collect()
}

#[derive(Debug, serde::Serialize)]
pub struct MetricSummary {
    pub cpu_pct: String,
    pub core_count: usize,
    pub frequency: String,
    pub memory_used: String,
    pub memory_free: String,
    pub memory_total: String,
    pub memory_pct: String,
    pub disk_count: usize,
    pub network_count: usize,
    pub process_count: usize,
    pub rx_total: String,
    pub tx_total: String,
    pub rx_rate: String,
    pub tx_rate: String,
    pub gpu_count: usize,
    pub gpu_used: String,
    pub gpu_total: String,
    pub gpu_bandwidth: String,
    pub processes: Vec<ProcessRow>,
    pub core_bars: Vec<CoreBar>,
    pub disks: Vec<DiskCard>,
    pub networks: Vec<NetworkCard>,
    pub gpus: Vec<GpuCard>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProcessRow {
    pub pid: u32,
    pub name: String,
    pub cpu_pct: String,
    pub cpu_pct_num: f32,
    pub memory: String,
    pub memory_bytes: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct ProcessCard {
    pub pid: u32,
    pub name: String,
    pub cpu_pct: String,
    pub cpu_pct_num: f32,
    pub cpu_bar: f32,
    pub memory: String,
    pub memory_bytes: u64,
    pub memory_bar: f32,
    pub sparkline: Vec<f32>,
}

#[derive(Debug, serde::Serialize)]
pub struct CoreBar {
    pub number: usize,
    pub pct: f32,
}

#[derive(Debug, serde::Serialize)]
pub struct DiskCard {
    pub name: String,
    pub used: String,
    pub total: String,
    pub pct: f32,
    pub read_rate: String,
    pub write_rate: String,
    pub read_rate_num: f32,
    pub write_rate_num: f32,
    pub sparkline: Vec<f32>,
}

#[derive(Debug, serde::Serialize)]
pub struct SwapCard {
    pub used: String,
    pub total: String,
    pub pct: f32,
    pub used_num: u64,
    pub total_num: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct StorageEventRow {
    pub ts: String,
    pub source: String,
    pub severity: String,
    pub severity_class: String,
    pub message: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ProcessEventRow {
    pub ts: String,
    pub source: String,
    pub severity: String,
    pub severity_class: String,
    pub message: String,
}

#[derive(Debug, serde::Serialize)]
pub struct NetworkCard {
    pub name: String,
    pub rx: String,
    pub tx: String,
    pub rx_rate: String,
    pub tx_rate: String,
    pub rx_rate_num: f32,
    pub tx_rate_num: f32,
    pub rx_rate_bar: f32,
    pub tx_rate_bar: f32,
    pub sparkline: Vec<f32>,
}

#[derive(Debug, serde::Serialize)]
pub struct NetworkCombined {
    pub rx_total: u64,
    pub tx_total: u64,
    pub rx_rate_total: f32,
    pub tx_rate_total: f32,
    pub rx_total_str: String,
    pub tx_total_str: String,
    pub rx_rate_str: String,
    pub tx_rate_str: String,
}

#[derive(Debug, serde::Serialize)]
pub struct NetworkTrafficRow {
    pub ts: String,
    pub source: String,
    pub severity: String,
    pub severity_class: String,
    pub message: String,
}

#[derive(Debug, serde::Serialize)]
pub struct GpuCard {
    pub name: String,
    pub usage_percent: f32,
    pub memory_used: String,
    pub memory_total: String,
    pub memory_pct: f32,
    pub bandwidth: String,
}

#[derive(Debug, serde::Serialize)]
pub struct AiServerCard {
    pub kind: String,
    pub name: String,
    pub pid: u32,
    pub cpu_pct: String,
    pub cpu_pct_num: f32,
    pub memory: String,
    pub memory_bytes: u64,
    pub port_hint: Option<u16>,
    pub endpoint: Option<String>,
    pub models: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct LogRow {
    pub ts: String,
    pub severity: String,
    pub severity_class: String,
    pub source: String,
    pub unit: String,
    pub message: String,
}

#[derive(Debug, serde::Serialize)]
pub struct SecurityAlertRow {
    pub ts: String,
    pub severity: String,
    pub severity_class: String,
    pub source: String,
    pub unit: String,
    pub message: String,
    pub hash: String,
    pub previous_hash: Option<String>,
}

impl MetricSummary {
    pub fn from_snapshot(m: &MetricSnapshot) -> Self {
        let rx_total: u64 = m.networks.iter().map(|n| n.rx_bytes).sum();
        let tx_total: u64 = m.networks.iter().map(|n| n.tx_bytes).sum();
        let rx_rate_total: f32 = m.networks.iter().map(|n| n.rx_rate).sum();
        let tx_rate_total: f32 = m.networks.iter().map(|n| n.tx_rate).sum();
        let gpu_used: u64 = m.gpu.iter().map(|g| g.memory_used_bytes).sum();
        let gpu_total: u64 = m.gpu.iter().map(|g| g.memory_total_bytes).sum();
        let gpu_bandwidth: f32 = m.gpu.iter().map(|g| g.bandwidth_bytes_per_sec).sum();
        Self {
            cpu_pct: format!("{:.1}%", m.cpu.usage_percent),
            core_count: m.cpu.per_core_usage.len(),
            frequency: format!("{} MHz", m.cpu.frequency_mhz),
            memory_used: format_bytes(m.memory.used_bytes),
            memory_free: format_bytes(m.memory.free_bytes),
            memory_total: format_bytes(m.memory.total_bytes),
            memory_pct: observa_shared::memory_pct(m.memory.used_bytes, m.memory.total_bytes),
            disk_count: m.disks.len(),
            network_count: m.networks.len(),
            process_count: m.processes.len(),
            rx_total: format_bytes(rx_total),
            tx_total: format_bytes(tx_total),
            rx_rate: format_rate(rx_rate_total),
            tx_rate: format_rate(tx_rate_total),
            gpu_count: m.gpu.len(),
            gpu_used: format_bytes(gpu_used),
            gpu_total: format_bytes(gpu_total),
            gpu_bandwidth: format_rate(gpu_bandwidth),
            processes: m.processes.iter().map(ProcessRow::from_process).collect(),
            core_bars: m
                .cpu
                .per_core_usage
                .iter()
                .enumerate()
                .map(|(i, v)| CoreBar {
                    number: i + 1,
                    pct: *v,
                })
                .collect(),
            disks: m.disks.iter().map(DiskCard::from_disk).collect(),
            networks: m.networks.iter().map(NetworkCard::from_network).collect(),
            gpus: m.gpu.iter().map(GpuCard::from_gpu).collect(),
        }
    }
}

impl DiskCard {
    pub fn from_disk(d: &DiskMetrics) -> Self {
        let pct = if d.total_bytes == 0 {
            0.0
        } else {
            100.0 * d.used_bytes as f32 / d.total_bytes as f32
        };
        Self {
            name: d.name.clone(),
            used: format_bytes(d.used_bytes),
            total: format_bytes(d.total_bytes),
            pct,
            read_rate: format_rate(d.read_bytes_per_sec),
            write_rate: format_rate(d.write_bytes_per_sec),
            read_rate_num: d.read_bytes_per_sec,
            write_rate_num: d.write_bytes_per_sec,
            sparkline: storage_sparkline(d.read_bytes_per_sec, d.write_bytes_per_sec),
        }
    }
}

impl SwapCard {
    pub fn from_snapshot(m: &MetricSnapshot) -> Option<Self> {
        let s = m.swap.as_ref()?;
        let pct = if s.total_bytes == 0 {
            0.0
        } else {
            100.0 * s.used_bytes as f32 / s.total_bytes as f32
        };
        Some(Self {
            used: format_bytes(s.used_bytes),
            total: format_bytes(s.total_bytes),
            pct,
            used_num: s.used_bytes,
            total_num: s.total_bytes,
        })
    }
}

impl NetworkCard {
    pub fn from_network(n: &NetworkMetrics) -> Self {
        const MAX_RATE: f32 = 100_000_000.0; // 100 MB/s reference for bar scaling
        Self {
            name: n.interface.clone(),
            rx: format_bytes(n.rx_bytes),
            tx: format_bytes(n.tx_bytes),
            rx_rate: format_rate(n.rx_rate),
            tx_rate: format_rate(n.tx_rate),
            rx_rate_num: n.rx_rate,
            tx_rate_num: n.tx_rate,
            rx_rate_bar: (n.rx_rate / MAX_RATE * 100.0).clamp(0.0, 100.0),
            tx_rate_bar: (n.tx_rate / MAX_RATE * 100.0).clamp(0.0, 100.0),
            sparkline: network_sparkline(n.rx_rate, n.tx_rate),
        }
    }
}

impl NetworkCombined {
    pub fn from_networks(networks: &[NetworkMetrics]) -> Self {
        let rx_total: u64 = networks.iter().map(|n| n.rx_bytes).sum();
        let tx_total: u64 = networks.iter().map(|n| n.tx_bytes).sum();
        let rx_rate_total: f32 = networks.iter().map(|n| n.rx_rate).sum();
        let tx_rate_total: f32 = networks.iter().map(|n| n.tx_rate).sum();
        Self {
            rx_total,
            tx_total,
            rx_rate_total,
            tx_rate_total,
            rx_total_str: format_bytes(rx_total),
            tx_total_str: format_bytes(tx_total),
            rx_rate_str: format_rate(rx_rate_total),
            tx_rate_str: format_rate(tx_rate_total),
        }
    }
}

impl AiServerCard {
    pub fn from_ai_server(s: &AiServerMetrics) -> Self {
        let endpoint = s.endpoint.clone().or_else(|| s.port_hint.map(|p| format!("http://127.0.0.1:{}", p)));
        Self {
            kind: format!("{:?}", s.kind),
            name: s.name.clone(),
            pid: s.pid,
            cpu_pct: format!("{:.1}%", s.cpu_percent),
            cpu_pct_num: s.cpu_percent,
            memory: format_bytes(s.memory_bytes),
            memory_bytes: s.memory_bytes,
            port_hint: s.port_hint,
            endpoint,
            models: s.models.clone(),
        }
    }
}

impl GpuCard {
    pub fn from_gpu(g: &GpuMetrics) -> Self {
        let pct = if g.memory_total_bytes == 0 {
            0.0
        } else {
            100.0 * g.memory_used_bytes as f32 / g.memory_total_bytes as f32
        };
        Self {
            name: g.name.clone(),
            usage_percent: g.usage_percent,
            memory_used: format_bytes(g.memory_used_bytes),
            memory_total: format_bytes(g.memory_total_bytes),
            memory_pct: pct,
            bandwidth: format_rate(g.bandwidth_bytes_per_sec),
        }
    }
}

impl ProcessRow {
    pub fn from_process(p: &ProcessMetrics) -> Self {
        Self {
            pid: p.pid,
            name: p.name.clone(),
            cpu_pct: format!("{:.1}%", p.cpu_percent),
            cpu_pct_num: p.cpu_percent,
            memory: format_bytes(p.memory_bytes),
            memory_bytes: p.memory_bytes,
        }
    }
}

impl ProcessCard {
    pub fn from_process(p: &ProcessMetrics) -> Self {
        const MAX_CPU: f32 = 100.0;
        const MAX_MEM: f32 = 8_000_000_000.0; // 8 GB reference for bar scaling
        Self {
            pid: p.pid,
            name: p.name.clone(),
            cpu_pct: format!("{:.1}%", p.cpu_percent),
            cpu_pct_num: p.cpu_percent,
            cpu_bar: p.cpu_percent.clamp(0.0, MAX_CPU),
            memory: format_bytes(p.memory_bytes),
            memory_bytes: p.memory_bytes,
            memory_bar: ((p.memory_bytes as f32 / MAX_MEM) * 100.0).clamp(0.0, 100.0),
            sparkline: process_sparkline(p.cpu_percent, p.memory_bytes as f32),
        }
    }
}

fn event_row_from_log(l: LogEvent) -> (String, String, String, String, String) {
    (
        l.ts.format("%H:%M:%S").to_string(),
        l.source,
        format!("{:?}", l.severity),
        severity_class(l.severity).to_string(),
        l.message,
    )
}

impl NetworkTrafficRow {
    pub fn from_log(l: LogEvent) -> Self {
        let (ts, source, severity, severity_class, message) = event_row_from_log(l);
        Self {
            ts,
            source,
            severity,
            severity_class,
            message,
        }
    }
}

impl ProcessEventRow {
    pub fn from_log(l: LogEvent) -> Self {
        let (ts, source, severity, severity_class, message) = event_row_from_log(l);
        Self {
            ts,
            source,
            severity,
            severity_class,
            message,
        }
    }
}

impl StorageEventRow {
    pub fn from_log(l: LogEvent) -> Self {
        let (ts, source, severity, severity_class, message) = event_row_from_log(l);
        Self {
            ts,
            source,
            severity,
            severity_class,
            message,
        }
    }
}

impl LogRow {
    pub fn from_event(l: &LogEvent) -> Self {
        Self {
            ts: l.ts.to_rfc3339(),
            severity: format!("{:?}", l.severity),
            severity_class: severity_class(l.severity).to_string(),
            source: l.source.clone(),
            unit: l.unit.clone().unwrap_or_default(),
            message: l.message.clone(),
        }
    }
}

impl SecurityAlertRow {
    pub fn from_alert(a: &SecurityAlert) -> Self {
        Self {
            ts: a.ts.to_rfc3339(),
            severity: format!("{:?}", a.severity),
            severity_class: severity_class(a.severity).to_string(),
            source: a.source.clone(),
            unit: a.unit.clone().unwrap_or_default(),
            message: a.message.clone(),
            hash: a.hash.clone(),
            previous_hash: a.previous_hash.clone(),
        }
    }
}
