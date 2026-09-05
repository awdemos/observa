use std::sync::Arc;
use std::time::{Duration, Instant};

use observa_bus::Bus;
use observa_config::{AiServerConfig, Config};
use observa_shared::{AiServerEvent, AiServerKind, AiServerMetrics, AiServerStatus, Event};
use tokio::sync::{watch, RwLock};
use tracing::{debug, warn};

/// Shared cache of remote AI server endpoint state.
pub type AiServerCache = Arc<RwLock<Vec<AiServerMetrics>>>;

/// Spawn a background task that probes configured AI inference endpoints.
pub fn spawn_ai_server_poller(
    config: Arc<Config>,
    cache: AiServerCache,
    bus: Bus,
    shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if config.ai_servers.is_empty() {
            return;
        }

        let client = match reqwest::Client::builder()
            .timeout(Duration::from_millis(config.ai_server_probe_timeout_ms))
            .connect_timeout(Duration::from_secs(2))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                warn!(%error, "failed to build AI server probe client");
                return;
            }
        };

        let interval = Duration::from_millis(config.ai_server_poll_interval_ms);
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Initial probe immediately.
        run_probe_round(&client, &config, &cache, &bus).await;

        loop {
            ticker.tick().await;
            if *shutdown.borrow() {
                break;
            }
            run_probe_round(&client, &config, &cache, &bus).await;
        }
    })
}

async fn run_probe_round(
    client: &reqwest::Client,
    config: &Config,
    cache: &AiServerCache,
    bus: &Bus,
) {
    let mut results = Vec::with_capacity(config.ai_servers.len());

    for cfg in &config.ai_servers {
        let result = probe_endpoint(client, cfg).await;
        results.push(result);
    }

    let previous: Vec<AiServerMetrics> = cache.read().await.clone();
    *cache.write().await = results.clone();

    for (prev, curr) in previous.iter().zip(results.iter()) {
        if prev.status != curr.status {
            let event = Event::AiServer(AiServerEvent {
                endpoint: curr.endpoint.clone().unwrap_or_else(|| curr.name.clone()),
                status: curr.status,
            });
            if let Err(error) = bus.publish(event) {
                warn!(%error, "failed to publish ai-server event");
            } else {
                debug!(endpoint = %curr.name, status = ?curr.status, "published ai-server event");
            }
        }
    }
}

async fn probe_endpoint(client: &reqwest::Client, cfg: &AiServerConfig) -> AiServerMetrics {
    let url = format!("{}/models", cfg.endpoint.trim_end_matches('/'));
    let name = cfg.name.clone().unwrap_or_else(|| cfg.endpoint.clone());
    let mut kind = cfg
        .kind
        .as_deref()
        .and_then(parse_kind)
        .unwrap_or(AiServerKind::Generic);
    let is_exo_hint = kind == AiServerKind::Exo
        || cfg.endpoint.to_lowercase().contains("exo")
        || cfg.endpoint.contains(":52415");

    let start = Instant::now();
    match client.get(&url).send().await {
        Ok(response) if response.status().is_success() => {
            let latency_ms = start.elapsed().as_millis() as u64;
            let models = response
                .json::<ModelListResponse>()
                .await
                .map(|r| r.data.into_iter().map(|m| m.id).collect())
                .unwrap_or_default();
            let cluster_nodes = if is_exo_hint {
                probe_exo_state(client, &cfg.endpoint).await
            } else {
                None
            };
            if kind == AiServerKind::Generic && cluster_nodes.is_some() {
                kind = AiServerKind::Exo;
            }
            AiServerMetrics {
                pid: None,
                kind,
                name,
                port_hint: None,
                endpoint: Some(cfg.endpoint.clone()),
                status: AiServerStatus::Online,
                latency_ms: Some(latency_ms),
                models,
                cluster_nodes,
                last_error: None,
                cpu_percent: 0.0,
                memory_bytes: 0,
            }
        }
        Ok(response) => AiServerMetrics {
            pid: None,
            kind,
            name,
            port_hint: None,
            endpoint: Some(cfg.endpoint.clone()),
            status: AiServerStatus::Offline,
            latency_ms: None,
            models: Vec::new(),
            cluster_nodes: None,
            last_error: Some(format!("HTTP {}", response.status())),
            cpu_percent: 0.0,
            memory_bytes: 0,
        },
        Err(error) => AiServerMetrics {
            pid: None,
            kind,
            name,
            port_hint: None,
            endpoint: Some(cfg.endpoint.clone()),
            status: AiServerStatus::Offline,
            latency_ms: None,
            models: Vec::new(),
            cluster_nodes: None,
            last_error: Some(error.to_string()),
            cpu_percent: 0.0,
            memory_bytes: 0,
        },
    }
}

fn parse_kind(s: &str) -> Option<AiServerKind> {
    match s.to_lowercase().as_str() {
        "vllm" => Some(AiServerKind::Vllm),
        "ollama" => Some(AiServerKind::Ollama),
        "triton" => Some(AiServerKind::Triton),
        "openai" => Some(AiServerKind::OpenAi),
        "sglang" => Some(AiServerKind::Sglang),
        "llamacpp" | "llama.cpp" => Some(AiServerKind::LlamaCpp),
        "exllamav2" | "exllama" => Some(AiServerKind::ExllamaV2),
        "koboldcpp" | "kobold" => Some(AiServerKind::KoboldCpp),
        "tabbyapi" | "tabby" => Some(AiServerKind::TabbyApi),
        "lmstudio" => Some(AiServerKind::LmStudio),
        "tgi" | "text_generation_inference" | "text-generation-inference" => {
            Some(AiServerKind::TextGenerationInference)
        }
        "exo" => Some(AiServerKind::Exo),
        _ => Some(AiServerKind::Generic),
    }
}

/// Probe an Exo Labs cluster master for its `/state` endpoint and extract the
/// IDs of nodes participating in the cluster.
async fn probe_exo_state(client: &reqwest::Client, endpoint: &str) -> Option<Vec<String>> {
    let base = strip_api_path(endpoint.trim_end_matches('/'));
    let url = format!("{}/state", base);
    let response = client.get(&url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let text = response.text().await.ok()?;
    let state: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mut nodes: Vec<String> = extract_node_ids(&state);
    nodes.sort();
    nodes.dedup();
    if nodes.is_empty() {
        None
    } else {
        Some(nodes)
    }
}

fn strip_api_path(url: &str) -> &str {
    for suffix in ["/v1", "/v2", "/api", "/api/tags", "/models"] {
        if let Some(stripped) = url.strip_suffix(suffix) {
            return stripped;
        }
    }
    url
}

fn extract_node_ids(state: &serde_json::Value) -> Vec<String> {
    let mut ids = Vec::new();

    if let Some(topology) = state.get("topology").or_else(|| state.get("Topology")) {
        if let Some(nodes) = topology.get("nodes").or_else(|| topology.get("Nodes")) {
            collect_node_keys(nodes, &mut ids);
        }
    }

    for key in ["node_identities", "node_memory", "node_disk", "node_system", "node_network", "last_seen"] {
        if let Some(map) = state.get(key).or_else(|| state.get(to_camel(key))) {
            collect_node_keys(map, &mut ids);
        }
    }

    ids
}

fn to_camel(s: &str) -> String {
    let mut parts = s.split('_');
    let first = parts.next().unwrap_or("");
    let rest: String = parts
        .map(|p| {
            let mut chars = p.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect();
    format!("{}{}", first, rest)
}

fn collect_node_keys(value: &serde_json::Value, out: &mut Vec<String>) {
    if let Some(obj) = value.as_object() {
        for key in obj.keys() {
            if !key.is_empty() {
                out.push(key.clone());
            }
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct ModelListResponse {
    data: Vec<ModelEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct ModelEntry {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kind_recognizes_known_backends() {
        assert_eq!(parse_kind("vllm"), Some(AiServerKind::Vllm));
        assert_eq!(parse_kind("Ollama"), Some(AiServerKind::Ollama));
        assert_eq!(parse_kind("unknown"), Some(AiServerKind::Generic));
    }

    #[test]
    fn model_list_response_extracts_ids() {
        let json = r#"{"object":"list","data":[{"id":"qwen2.5:7b","object":"model"},{"id":"llama3.1:latest","object":"model"}]}"#;
        let parsed: ModelListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.data.len(), 2);
        assert_eq!(parsed.data[0].id, "qwen2.5:7b");
        assert_eq!(parsed.data[1].id, "llama3.1:latest");
    }

    #[test]
    fn parse_kind_recognizes_exo() {
        assert_eq!(parse_kind("exo"), Some(AiServerKind::Exo));
        assert_eq!(parse_kind("Exo"), Some(AiServerKind::Exo));
    }

    #[test]
    fn extracts_exo_node_ids_from_state() {
        let json = r#"{
            "topology": {"nodes": {"node-1": {}, "node-2": {}}},
            "node_identities": {"node-3": {}}
        }"#;
        let state: serde_json::Value = serde_json::from_str(json).unwrap();
        let nodes = extract_node_ids(&state);
        assert_eq!(nodes.len(), 3);
        assert!(nodes.contains(&"node-1".to_string()));
        assert!(nodes.contains(&"node-2".to_string()));
        assert!(nodes.contains(&"node-3".to_string()));
    }
}
