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
    let kind = cfg
        .kind
        .as_deref()
        .and_then(parse_kind)
        .unwrap_or(AiServerKind::Generic);

    let start = Instant::now();
    match client.get(&url).send().await {
        Ok(response) if response.status().is_success() => {
            let latency_ms = start.elapsed().as_millis() as u64;
            let models = response
                .json::<ModelListResponse>()
                .await
                .map(|r| r.data.into_iter().map(|m| m.id).collect())
                .unwrap_or_default();
            AiServerMetrics {
                pid: None,
                kind,
                name,
                port_hint: None,
                endpoint: Some(cfg.endpoint.clone()),
                status: AiServerStatus::Online,
                latency_ms: Some(latency_ms),
                models,
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
        _ => Some(AiServerKind::Generic),
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
}
