use std::collections::HashMap;

use chrono::{DateTime, Utc};
use observa_shared::{
    AiServerKind, AiServerMetrics, CpuMetrics, DiskMetrics, MemoryMetrics, MetricSnapshot,
    NetworkMetrics, ProcessMetrics, SwapMetrics,
};
use sysinfo::{Disks, Networks, System};

type NetworkCounters = HashMap<String, (u64, u64, DateTime<Utc>)>;
static PREV_NETWORK: parking_lot::Mutex<Option<NetworkCounters>> = parking_lot::Mutex::new(None);

type DiskCounters = HashMap<String, (u64, u64, DateTime<Utc>)>;
static PREV_DISK: parking_lot::Mutex<Option<DiskCounters>> = parking_lot::Mutex::new(None);

/// Path to the host `/proc` directory inside the container.
const HOST_PROC: &str = "/host/proc";

/// Parse `/host/proc/meminfo` (or `$HOST_PROC/meminfo`) when available so the
/// dashboard reports host memory instead of the container's cgroup limit.
/// Returns `(total_bytes, used_bytes, swap_total_bytes, swap_free_bytes)`.
fn read_host_meminfo() -> Option<(u64, u64, u64, u64)> {
    let proc_root: std::path::PathBuf = std::env::var_os("HOST_PROC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| HOST_PROC.into());
    if !proc_root.is_dir() {
        return None;
    }
    let path = proc_root.join("meminfo");
    let text = std::fs::read_to_string(&path).ok()?;
    parse_meminfo_kb(&text).map(|(total_kb, available_kb, swap_total_kb, swap_free_kb)| {
        (
            total_kb * 1024,
            total_kb.saturating_sub(available_kb) * 1024,
            swap_total_kb * 1024,
            swap_free_kb * 1024,
        )
    })
}

fn parse_meminfo_kb(text: &str) -> Option<(u64, u64, u64, u64)> {
    let mut mem_total = None;
    let mut mem_available = None;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next()?;
        let value = parts.next().and_then(|s| s.parse::<u64>().ok())?;
        match key {
            "MemTotal:" => mem_total = Some(value),
            "MemAvailable:" => mem_available = Some(value),
            "SwapTotal:" => swap_total = value,
            "SwapFree:" => swap_free = value,
            _ => {}
        }
    }
    Some((mem_total?, mem_available?, swap_total, swap_free))
}

/// Deduplicate disks that are bind mounts or overlays of the same underlying
/// filesystem, keeping the most descriptive device name.
fn deduplicate_disks(disks: Vec<DiskMetrics>) -> Vec<DiskMetrics> {
    let mut by_total: HashMap<u64, DiskMetrics> = HashMap::new();
    for disk in disks {
        // Skip autofs / unresolvable entries that report zero total space.
        if disk.total_bytes == 0 {
            continue;
        }
        by_total
            .entry(disk.total_bytes)
            .and_modify(|existing| {
                if is_better_disk_name(&disk.name, &existing.name) {
                    existing.name = disk.name.clone();
                }
            })
            .or_insert(disk);
    }
    by_total.into_values().collect()
}

fn is_better_disk_name(new: &str, old: &str) -> bool {
    if new == old {
        return false;
    }
    let new_real = new.starts_with("/dev/");
    let old_real = old.starts_with("/dev/");
    if new_real && !old_real {
        return true;
    }
    if !new_real && old_real {
        return false;
    }
    // Prefer shorter, more generic names for virtual filesystems.
    new.len() < old.len()
}

/// Convert a refreshed `sysinfo::System` into Observa's `MetricSnapshot`.
///
/// The caller must have called `System::refresh_all()` (or the equivalent
/// sequence) before invoking this function, otherwise CPU usage values will be
/// zero on the first sample.
pub fn normalize(system: &System) -> MetricSnapshot {
    let cpus = system.cpus();
    let per_core_usage: Vec<f32> = cpus.iter().map(|cpu| cpu.cpu_usage()).collect();
    let usage_percent = system.global_cpu_usage();

    // sysinfo does not expose a single CPU frequency on all platforms; report 0
    // when unavailable rather than failing.
    let frequency_mhz = cpus.first().map(|cpu| cpu.frequency()).unwrap_or(0);

    // Prefer host /proc/meminfo when Observa runs inside a container, otherwise
    // sysinfo reports the container's cgroup memory limit.
    let (memory, swap) = if let Some((total, used, swap_total, swap_free)) = read_host_meminfo() {
        let memory = MemoryMetrics {
            total_bytes: total,
            used_bytes: used,
            free_bytes: total.saturating_sub(used),
        };
        let swap = if swap_total > 0 {
            Some(SwapMetrics {
                total_bytes: swap_total,
                used_bytes: swap_total.saturating_sub(swap_free),
                free_bytes: swap_free,
            })
        } else {
            None
        };
        (memory, swap)
    } else {
        let memory = MemoryMetrics {
            total_bytes: system.total_memory(),
            used_bytes: system.used_memory(),
            free_bytes: system.total_memory().saturating_sub(system.used_memory()),
        };

        let swap = if system.total_swap() > 0 {
            Some(SwapMetrics {
                total_bytes: system.total_swap(),
                used_bytes: system.used_swap(),
                free_bytes: system.total_swap().saturating_sub(system.used_swap()),
            })
        } else {
            None
        };
        (memory, swap)
    };

    // sysinfo sees every bind mount as a separate disk inside a container.
    // Deduplicate by total size so the dashboard shows one entry per physical
    // filesystem (e.g. the host disk backing the container overlay).
    let disks = deduplicate_disks(
        Disks::new_with_refreshed_list()
            .iter()
            .map(|disk| {
                let name = disk.name().to_string_lossy().into_owned();
                let usage = disk.usage();
                let read_bytes = usage.total_read_bytes;
                let written_bytes = usage.total_written_bytes;
                let total = disk.total_space();
                let used = total.saturating_sub(disk.available_space());
                let mut read_rate = 0.0f32;
                let mut write_rate = 0.0f32;
                {
                    let mut store = PREV_DISK.lock();
                    let now = Utc::now();
                    let prev = store.as_ref().and_then(|m| m.get(&name)).copied();
                    if let Some((prev_read, prev_write, prev_ts)) = prev {
                        let secs = (now - prev_ts).num_milliseconds().max(1) as f32 / 1000.0;
                        read_rate = read_bytes.saturating_sub(prev_read) as f32 / secs;
                        write_rate = written_bytes.saturating_sub(prev_write) as f32 / secs;
                    }
                    store
                        .get_or_insert_with(HashMap::new)
                        .insert(name.clone(), (read_bytes, written_bytes, now));
                }
                DiskMetrics {
                    name,
                    total_bytes: total,
                    used_bytes: used,
                    read_bytes_per_sec: read_rate,
                    write_bytes_per_sec: write_rate,
                }
            })
            .collect(),
    );

    let networks = Networks::new_with_refreshed_list()
        .iter()
        .map(|(interface, data)| NetworkMetrics {
            interface: interface.to_string(),
            rx_bytes: data.total_received(),
            tx_bytes: data.total_transmitted(),
            rx_rate: 0.0,
            tx_rate: 0.0,
        })
        .map(|mut n| {
            let mut store = PREV_NETWORK.lock();
            let now = Utc::now();
            let prev = store.as_ref().and_then(|m| m.get(&n.interface)).copied();
            if let Some((prev_rx, prev_tx, prev_ts)) = prev {
                let secs = (now - prev_ts).num_milliseconds().max(1) as f32 / 1000.0;
                let rx_delta = n.rx_bytes.saturating_sub(prev_rx) as f32;
                let tx_delta = n.tx_bytes.saturating_sub(prev_tx) as f32;
                n.rx_rate = rx_delta / secs;
                n.tx_rate = tx_delta / secs;
            }
            store
                .get_or_insert_with(HashMap::new)
                .insert(n.interface.clone(), (n.rx_bytes, n.tx_bytes, now));
            drop(store);
            n
        })
        .collect();

    // Collect all processes first so AI server detection sees inference engines
    // even when they are idle, then truncate the stored payload.
    let mut processes: Vec<_> = system
        .processes()
        .iter()
        .map(|(pid, process)| ProcessMetrics {
            pid: pid.as_u32(),
            name: process.name().to_string_lossy().into_owned(),
            cmdline: {
                let cmd = process.cmd();
                if cmd.is_empty() {
                    None
                } else {
                    Some(cmd.iter().map(|c| c.to_string_lossy()).collect::<Vec<_>>().join(" "))
                }
            },
            cpu_percent: process.cpu_usage(),
            memory_bytes: process.memory(),
        })
        .collect();

    let ai_servers = detect_ai_servers(&processes);

    processes.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    processes.truncate(25);

    let gpu = collect_gpu();

    MetricSnapshot {
        ts: Utc::now(),
        cpu: CpuMetrics {
            usage_percent,
            per_core_usage,
            frequency_mhz,
        },
        memory,
        swap,
        disks,
        networks,
        processes,
        gpu,
        ai_servers,
    }
}

fn detect_ai_servers(processes: &[ProcessMetrics]) -> Vec<AiServerMetrics> {
    processes
        .iter()
        .filter_map(|p| {
            let name_lower = p.name.to_lowercase();
            let cmd_lower = p.cmdline.as_deref().unwrap_or("").to_lowercase();
            let combined = format!("{} {}", name_lower, cmd_lower);
            let kind = if name_lower.contains("llama-server")
                || name_lower.contains("koboldcpp")
                || name_lower.contains("llamacpp")
                || combined.contains("llama.cpp")
            {
                Some(AiServerKind::LlamaCpp)
            } else if name_lower.contains("vllm") || combined.contains("vllm") {
                Some(AiServerKind::Vllm)
            } else if name_lower.contains("ollama") {
                Some(AiServerKind::Ollama)
            } else if name_lower.contains("tritonserver")
                || name_lower.contains("triton inference")
            {
                Some(AiServerKind::Triton)
            } else if name_lower.contains("openai-server")
                || name_lower.contains("openai_server")
                || name_lower.contains("openai-server")
            {
                Some(AiServerKind::OpenAi)
            } else if name_lower.contains("sglang") || combined.contains("sglang") {
                Some(AiServerKind::Sglang)
            } else if name_lower.contains("exllamav2") || combined.contains("exllama") {
                Some(AiServerKind::ExllamaV2)
            } else if name_lower.contains("tabbyapi") || combined.contains("tabbyapi") {
                Some(AiServerKind::TabbyApi)
            } else if name_lower.contains("lmstudio") || combined.contains("lm studio") {
                Some(AiServerKind::LmStudio)
            } else if name_lower.contains("text-generation-inference")
                || name_lower.contains("text_generation_inference")
                || combined.contains("text-generation-inference")
            {
                Some(AiServerKind::TextGenerationInference)
            } else if (name_lower.starts_with("python") || name_lower.starts_with("uvicorn") || name_lower.starts_with("fastapi"))
                && contains_any(&combined, &[
                    "vllm",
                    "ollama",
                    "triton",
                    "openai",
                    "sglang",
                    "llama",
                    "exllama",
                    "tabby",
                    "kobold",
                    "tgi",
                    "text-generation",
                    "transformers",
                    "flask",
                    "fastapi",
                    "gradio",
                    "streamlit",
                ])
                || combined.contains("model server")
                || combined.contains("llm-server")
                || combined.contains("inference server")
                || combined.contains("llm server")
            {
                Some(AiServerKind::Generic)
            } else {
                None
            };
            kind.map(|kind| AiServerMetrics {
                pid: p.pid,
                kind,
                name: p.name.clone(),
                port_hint: None,
                endpoint: None,
                models: Vec::new(),
                cpu_percent: p.cpu_percent,
                memory_bytes: p.memory_bytes,
            })
        })
        .collect()
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn collect_gpu() -> Vec<observa_shared::GpuMetrics> {
    let output = match std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,pcie.link.gen.max,pcie.link.width.max",
            "--format=csv,noheader,nounits",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };

    parse_gpu_output(&String::from_utf8_lossy(&output))
}

fn parse_gpu_output(text: &str) -> Vec<observa_shared::GpuMetrics> {
    text.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() < 2 {
                return None;
            }
            let name = parts[0].trim();
            if name.is_empty() {
                return None;
            }
            let mut parts = parts.into_iter();
            let _name = parts.next()?;
            let usage_percent = parts.next().and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(0.0);
            let memory_used_mib = parts.next().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
            let memory_used_bytes = memory_used_mib * 1024 * 1024;
            let memory_total_mib = parts.next().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
            let memory_total_bytes = memory_total_mib * 1024 * 1024;
            let pcie_gen = parts.next().and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(0.0);
            let pcie_width = parts.next().and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(0.0);
            // nvidia-smi reports memory in MiB and PCIe gen/width; convert to bytes/s.
            let lane_speed_gbps = match pcie_gen as u8 {
                1 => 0.25,
                2 => 0.5,
                3 => 1.0,
                4 => 2.0,
                5 => 4.0,
                6 => 8.0,
                _ => 0.0,
            };
            let bandwidth_bytes_per_sec = lane_speed_gbps * pcie_width * 2.0 * 1_000_000_000.0;
            Some(observa_shared::GpuMetrics {
                name: name.to_string(),
                usage_percent,
                memory_used_bytes,
                memory_total_bytes,
                bandwidth_bytes_per_sec,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_meminfo_kb_extracts_host_memory_and_swap() {
        let text = r#"MemTotal:       65536000 kB
MemFree:         8192000 kB
MemAvailable:   52428800 kB
Buffers:          102400 kB
SwapTotal:       4194304 kB
SwapFree:        2097152 kB
"#;
        let (total, available, swap_total, swap_free) = parse_meminfo_kb(text).unwrap();
        assert_eq!(total, 65_536_000);
        assert_eq!(available, 52_428_800);
        assert_eq!(swap_total, 4_194_304);
        assert_eq!(swap_free, 2_097_152);
    }

    #[test]
    fn parse_meminfo_kb_returns_none_without_memtotal() {
        let text = "MemAvailable: 52428800 kB\n";
        assert!(parse_meminfo_kb(text).is_none());
    }

    #[test]
    fn parse_gpu_output_extracts_name_and_utilization() {
        let text = "NVIDIA GeForce RTX 4090, 35, 4096, 24576, 4, 16\nNVIDIA RTX A4000, 12, 2048, 16384, 4, 16";
        let gpus = parse_gpu_output(text);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(gpus[0].usage_percent, 35.0);
        assert_eq!(gpus[0].memory_used_bytes, 4_096 * 1_024 * 1_024);
        assert_eq!(gpus[0].memory_total_bytes, 24_576 * 1_024 * 1_024);
        assert!(gpus[0].bandwidth_bytes_per_sec > 0.0);
        assert_eq!(gpus[1].name, "NVIDIA RTX A4000");
        assert_eq!(gpus[1].usage_percent, 12.0);
    }

    #[test]
    fn parse_gpu_output_keeps_lines_with_missing_utilization_as_zero() {
        let text = "NVIDIA A100, 7, 8192, 81920, 4, 16\nmalformed-without-comma\nNVIDIA T4, 0, 0, 0,,";
        let gpus = parse_gpu_output(text);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "NVIDIA A100");
        assert_eq!(gpus[0].usage_percent, 7.0);
        assert_eq!(gpus[1].name, "NVIDIA T4");
        assert_eq!(gpus[1].usage_percent, 0.0);
    }

    #[test]
    fn parse_gpu_output_accepts_na_memory_values() {
        let text = "NVIDIA GB10, 6, [N/A], [N/A], 1, 16";
        let gpus = parse_gpu_output(text);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].name, "NVIDIA GB10");
        assert_eq!(gpus[0].usage_percent, 6.0);
        assert_eq!(gpus[0].memory_used_bytes, 0);
        assert_eq!(gpus[0].memory_total_bytes, 0);
    }

    #[test]
    fn parse_gpu_output_skips_completely_empty_lines() {
        let text = "NVIDIA T4, 0, 0, 0, 0, 0\n\n   \n";
        let gpus = parse_gpu_output(text);
        assert_eq!(gpus.len(), 1);
    }

    #[test]
    fn parse_gpu_output_computes_pcie_bandwidth() {
        let text = "NVIDIA RTX A4000, 12, 2048, 16384, 4, 16";
        let gpus = parse_gpu_output(text);
        assert_eq!(gpus.len(), 1);
        // Gen4 = 2 GB/s per lane duplex, 16 lanes: 64 GB/s.
        assert_eq!(gpus[0].bandwidth_bytes_per_sec, 64.0 * 1_000_000_000.0);
    }
}
