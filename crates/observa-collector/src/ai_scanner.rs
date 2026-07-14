use std::collections::HashSet;
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::Duration;

use observa_shared::{AiServerKind, AiServerMetrics};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info};

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1000);

/// Common ports used by inference engines.  Scanned on localhost and, when
/// permitted, on the local subnet.
const INFERENCE_PORTS: &[u16] = &[
    8000,  // vLLM, Triton, FastAPI, TGI
    8080,  // llama.cpp, generic
    8888,  // Jupyter/Gradio/Streamlit wrappers
    9000,  // custom model servers
    11434, // Ollama
    1234,  // LM Studio
    5000,  // TabbyAPI, Flask
    30000, // SGLang
    8001,  // Triton HTTP secondary
    8002,  // Triton gRPC (not HTTP, but listed for completeness)
    5001,  // common alternate
    3000,  // Next.js/Node wrappers
    7860,  // Gradio default
    8501,  // Streamlit default
    10000, // Modal/RunPod common
    18080, // Observa llama-server host mapping
    4000,  // common dev / alternate
    7000,  // common alternate
    9090,  // common alternate / Prometheus exporters
];

/// Endpoints that may expose model lists.  OpenAI-compatible APIs use
/// `/v1/models`; Triton uses `/v2/models`; some custom servers expose `/models`.
const MODELS_PATHS: &[&str] = &["/v1/models", "/v2/models", "/models"];
const OLLAMA_TAGS_PATH: &str = "/api/tags";

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
    #[serde(default)]
    models: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    id: Option<String>,
    #[serde(rename = "name")]
    name_field: Option<String>,
    #[serde(rename = "model")]
    model_field: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    #[serde(default)]
    models: Vec<OllamaModel>,
}

#[derive(Debug, Deserialize)]
struct OllamaModel {
    name: Option<String>,
    model: Option<String>,
}

/// Discover AI inference servers from a list of explicit endpoints plus a
/// lightweight scan of common localhost ports and the local subnet.
///
/// `endpoints` may be bare `host:port` strings or full URL prefixes.  The
/// scanner is designed to be safe on shared networks: it only probes a small
/// set of well-known inference ports and stops at the first responsive path.
pub async fn discover_ai_servers(endpoints: &[String], subnet_scan_enabled: bool) -> Vec<AiServerMetrics> {
    info!(endpoint_count = endpoints.len(), subnet_scan = subnet_scan_enabled, "discovering AI inference servers");
    let client = match build_client() {
        Some(c) => c,
        None => return Vec::new(),
    };

    let mut targets: Vec<(String, String)> = Vec::new();

    // 1. Explicitly configured endpoints.
    for endpoint in endpoints {
        let (base_url, label) = normalize_endpoint(endpoint);
        targets.push((base_url, label));
    }

    // 2. Common localhost ports.
    for port in INFERENCE_PORTS {
        let label = format!("http://127.0.0.1:{}", port);
        targets.push((label.clone(), label));
    }

    // 3. Local subnet scan for common inference ports (opt-in).
    if subnet_scan_enabled {
        if let Some(subnet) = local_subnet() {
            let subnet_targets = enumerate_subnet_targets(&subnet);
            targets.extend(subnet_targets);
        }
    }

    let results = probe_targets(&client, targets).await;
    for server in &results {
        log_detected(server);
    }

    if results.is_empty() {
        info!("no AI inference servers discovered");
    } else {
        info!(server_count = results.len(), "AI inference server discovery complete");
    }

    results
}

async fn probe_targets(client: &Client, targets: Vec<(String, String)>) -> Vec<AiServerMetrics> {
    let mut seen = HashSet::new();
    let mut unique_targets = Vec::new();
    for (base_url, label) in targets {
        if seen.insert(label.clone()) {
            unique_targets.push((base_url, label));
        }
    }

    let mut results: Vec<AiServerMetrics> = Vec::new();
    let mut set: tokio::task::JoinSet<Option<AiServerMetrics>> = tokio::task::JoinSet::new();

    for (base_url, label) in unique_targets {
        // Keep concurrency bounded so we do not overwhelm the local network.
        if set.len() >= 32 {
            if let Some(Ok(Some(server))) = set.join_next().await {
                results.push(server);
            }
        }
        let client = client.clone();
        set.spawn(async move { probe_endpoint(&client, &base_url, &label).await });
    }

    // Await every remaining task; ignore None/Err results but do not stop early.
    while let Some(result) = set.join_next().await {
        if let Ok(Some(server)) = result {
            results.push(server);
        }
    }

    results
}

fn build_client() -> Option<Client> {
    Client::builder()
        .timeout(PROBE_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .ok()
}

/// Strip a trailing API path segment so the endpoint can be probed at the
/// correct model-list path.  For example `http://host:8080/v1` becomes
/// `http://host:8080` and `http://host:8080/api/tags` becomes `http://host:8080`.
fn strip_api_path(url: &str) -> &str {
    for suffix in ["/v1", "/v2", "/api", "/api/tags", "/models"] {
        if let Some(stripped) = url.strip_suffix(suffix) {
            return stripped;
        }
    }
    url
}

fn normalize_endpoint(endpoint: &str) -> (String, String) {
    let trimmed = endpoint.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let base = strip_api_path(trimmed.trim_end_matches('/'));
        return (base.to_string(), base.to_string());
    }
    let base = format!("http://{}", trimmed);
    (base.clone(), base)
}

fn format_error_chain(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(err) = source {
        message.push_str(&format!(" <- {err}"));
        source = err.source();
    }
    message
}

async fn probe_endpoint(client: &Client, base_url: &str, label: &str) -> Option<AiServerMetrics> {
    // Try Ollama's native endpoint first so we can tag it as Ollama even when
    // the URL gives no hint.
    if let Some(server) = probe_ollama_tags(client, base_url, label).await {
        return Some(server);
    }

    for path in MODELS_PATHS {
        let url = format!("{}{}", base_url, path);
        match client.get(&url).send().await {
            Ok(response) if response.status().is_success() => {
                let status = response.status();
                let text = match response.text().await {
                    Ok(t) => t,
                    Err(e) => {
                        debug!(url = %url, error = %format_error_chain(&e), "AI server probe returned unreadable body");
                        continue;
                    }
                };
                debug!(url = %url, status = %status, body_len = text.len(), "AI server probe succeeded");

                // Some engines return OpenAI-shaped data in `data` or `models`.
                let body: ModelsResponse = match serde_json::from_str(&text) {
                    Ok(b) => b,
                    Err(e) => {
                        debug!(url = %url, error = %e, "response is not an OpenAI models list");
                        return Some(build_server(label, base_url, AiServerKind::Generic, Vec::new()));
                    }
                };

                let models = extract_model_ids(&body);
                let kind = infer_kind(base_url, &models);
                return Some(build_server(label, base_url, kind, models));
            }
            Ok(response) => {
                debug!(url = %url, status = %response.status(), "AI server probe returned non-success");
            }
            Err(e) => {
                debug!(url = %url, error = %format_error_chain(&e), "AI server probe failed");
            }
        }
    }
    None
}

async fn probe_ollama_tags(client: &Client, base_url: &str, label: &str) -> Option<AiServerMetrics> {
    let url = format!("{}{}", base_url, OLLAMA_TAGS_PATH);
    match client.get(&url).send().await {
        Ok(response) if response.status().is_success() => {
            let text = match response.text().await {
                Ok(t) => t,
                Err(e) => {
                    debug!(url = %url, error = %format_error_chain(&e), "Ollama probe returned unreadable body");
                    return None;
                }
            };
            let body: OllamaTagsResponse = serde_json::from_str(&text).ok()?;
            let models: Vec<String> = body
                .models
                .into_iter()
                .filter_map(|m| m.name.or(m.model))
                .filter(|s| !s.is_empty())
                .collect();
            if models.is_empty() {
                return None;
            }
            Some(build_server(label, base_url, AiServerKind::Ollama, models))
        }
        Ok(response) => {
            debug!(url = %url, status = %response.status(), "Ollama probe returned non-success");
            None
        }
        Err(e) => {
            debug!(url = %url, error = %format_error_chain(&e), "Ollama probe failed");
            None
        }
    }
}

fn extract_model_ids(body: &ModelsResponse) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for entry in body.data.iter().chain(body.models.iter()) {
        if let Some(id) = entry
            .id
            .clone()
            .or_else(|| entry.name_field.clone())
            .or_else(|| entry.model_field.clone())
        {
            if !id.is_empty() && seen.insert(id.clone()) {
                ids.push(id);
            }
        }
    }
    ids
}

fn infer_kind(base_url: &str, models: &[String]) -> AiServerKind {
    let lower = base_url.to_lowercase();
    let combined = format!("{} {}", lower, models.join(" ").to_lowercase());

    if contains_any(&lower, &["ollama"]) || models.iter().any(|m| m.contains(":") && !m.contains("/")) {
        return AiServerKind::Ollama;
    }
    if contains_any(&combined, &["llama-server", "llama.cpp", "llamacpp"])
        || models.iter().any(|m| m.ends_with(".gguf"))
    {
        return AiServerKind::LlamaCpp;
    }
    if contains_any(&lower, &["vllm"]) {
        return AiServerKind::Vllm;
    }
    if contains_any(&lower, &["tritonserver", "triton_server", "triton"]) {
        return AiServerKind::Triton;
    }
    if contains_any(&lower, &["sglang"]) {
        return AiServerKind::Sglang;
    }
    if contains_any(&lower, &["tabbyapi", "tabby"]) {
        return AiServerKind::TabbyApi;
    }
    if contains_any(&lower, &["lmstudio", "lm-studio"]) {
        return AiServerKind::LmStudio;
    }
    if contains_any(&lower, &[
        "text-generation-inference",
        "text_generation_inference",
        "text-generation",
        "tgi",
        "huggingface",
    ]) {
        return AiServerKind::TextGenerationInference;
    }
    if contains_any(&lower, &["exllama", "exllamav2"]) {
        return AiServerKind::ExllamaV2;
    }
    if contains_any(&lower, &["kobold", "koboldcpp"]) {
        return AiServerKind::KoboldCpp;
    }
    AiServerKind::Generic
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn build_server(display: &str, base_url: &str, kind: AiServerKind, models: Vec<String>) -> AiServerMetrics {
    let name = display
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_string();
    let port_hint = name
        .split(':')
        .nth(1)
        .and_then(|p| p.parse::<u16>().ok());

    AiServerMetrics {
        pid: 0,
        kind,
        name,
        port_hint,
        endpoint: Some(base_url.to_string()),
        models,
        cpu_percent: 0.0,
        memory_bytes: 0,
    }
}

fn log_detected(server: &AiServerMetrics) {
    info!(
        endpoint = %server.name,
        kind = ?server.kind,
        model_count = server.models.len(),
        "detected AI inference server"
    );
}

/// Best-effort discovery of the local subnet.  Returns the network address and
/// prefix length (e.g. "192.168.1.0/24") if a likely private interface is found.
///
/// We use a temporary UDP socket to a public address to learn the local IP
/// without sending any packets.
fn local_subnet() -> Option<String> {
    let local_ip = local_ipv4_via_socket()?;

    if is_private_ipv4(local_ip) {
        let net = network_address(local_ip, 24);
        return Some(format!("{}/24", net));
    }
    None
}

fn local_ipv4_via_socket() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let local = socket.local_addr().ok()?;
    match local.ip() {
        IpAddr::V4(ipv4) => Some(ipv4),
        _ => None,
    }
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    // 10.0.0.0/8
    octets[0] == 10
        // 172.16.0.0/12
        || (octets[0] == 172 && (octets[1] & 0xf0) == 16)
        // 192.168.0.0/16
        || (octets[0] == 192 && octets[1] == 168)
        // 127.0.0.0/8
        || octets[0] == 127
}

fn network_address(ip: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    let mask = u32::MAX << (32 - prefix);
    let ip_u32 = u32::from(ip);
    Ipv4Addr::from(ip_u32 & mask)
}

/// Enumerate likely inference-server URLs on a local subnet.  Only /24 subnets
/// are supported and private address space is enforced.
fn enumerate_subnet_targets(subnet: &str) -> Vec<(String, String)> {
    let (base, prefix_str) = subnet.split_once('/').unwrap_or((subnet, "24"));
    let prefix: u8 = prefix_str.parse().unwrap_or(24);
    if !(16..=30).contains(&prefix) {
        return Vec::new();
    }

    let network = match base.parse::<Ipv4Addr>() {
        Ok(ip) => ip,
        Err(_) => return Vec::new(),
    };

    let host_count = 1u32 << (32 - prefix);
    let network_u32 = u32::from(network);
    let mut targets = Vec::new();

    // Scan .1 through .254, skipping the network and broadcast addresses.
    for offset in 1..host_count.saturating_sub(1) {
        let ip = Ipv4Addr::from(network_u32 + offset);
        for port in INFERENCE_PORTS {
            let label = format!("http://{}:{}", ip, port);
            targets.push((label.clone(), label));
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_bare_host_port() {
        let (base, display) = normalize_endpoint("llama-server:8080");
        assert_eq!(base, "http://llama-server:8080");
        assert_eq!(display, "http://llama-server:8080");
    }

    #[test]
    fn normalizes_url_prefix() {
        let (base, display) = normalize_endpoint("http://localhost:8080/v1");
        assert_eq!(base, "http://localhost:8080");
        assert_eq!(display, "http://localhost:8080");
    }

    #[test]
    fn strips_api_path_from_url() {
        let (base, _) = normalize_endpoint("http://localhost:11434/api/tags");
        assert_eq!(base, "http://localhost:11434");
    }

    #[test]
    fn infers_llamacpp_from_gguf_model() {
        let kind = infer_kind("http://host:8080", &["model-Q4_K_M.gguf".to_string()]);
        assert_eq!(kind, AiServerKind::LlamaCpp);
    }

    #[test]
    fn infers_ollama_from_url() {
        let kind = infer_kind("http://ollama:11434", &[]);
        assert_eq!(kind, AiServerKind::Ollama);
    }

    #[test]
    fn infers_ollama_from_tagged_model() {
        let kind = infer_kind("http://127.0.0.1:11434", &["qwen:latest".to_string()]);
        assert_eq!(kind, AiServerKind::Ollama);
    }

    #[test]
    fn detects_private_network_address() {
        let net = network_address(Ipv4Addr::new(192, 168, 5, 10), 24);
        assert_eq!(net, Ipv4Addr::new(192, 168, 5, 0));
    }
}
