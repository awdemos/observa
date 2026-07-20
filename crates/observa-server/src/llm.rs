use std::time::Duration;

use observa_llm::LlmClient;
use observa_shared::{ChatMessage, ObservaError, Result};
use tokio::time::timeout as tokio_timeout;

use crate::state::AppState;

const AUTO_LLM_TIMEOUT_SECS: u64 = 60;

/// Build an LLM client from a discovered AI inference server.
///
/// When the operator has not configured an explicit LLM, Observa can still route chat
/// to a locally-running OpenAI-compatible server that the collector discovered (Ollama,
/// llama.cpp, vLLM, etc.). This avoids the hard-coded fallback responder in the common
/// homelab case where an inference engine is already running.
fn llm_client_from_discovered_server(server: &observa_shared::AiServerMetrics) -> Option<LlmClient> {
    let base = server.endpoint.as_deref()?;
    let base = base.trim_end_matches('/');
    // Collector stores bare host:port; chat client needs the OpenAI /v1 prefix.
    let base_url = format!("{base}/v1");
    let model = pick_chat_model(&server.models).unwrap_or_else(|| "llama".to_string());
    tracing::info!(
        endpoint = %base_url,
        model = %model,
        kind = ?server.kind,
        "auto-configuring LLM client from discovered AI server"
    );
    Some(LlmClient::new(
        base_url,
        Some("unused".to_string()),
        model,
        Some(Duration::from_secs(AUTO_LLM_TIMEOUT_SECS)),
    ))
}

/// Pick a chat-capable model from a server's model list.
///
/// Avoids vision and embedding models (e.g. moondream, bge-m3) that return empty or
/// nonsensical answers for text chat. Prefers well-known instruction/chat families.
fn pick_chat_model(models: &[String]) -> Option<String> {
    let lower = models.iter().map(|m| m.to_lowercase()).collect::<Vec<_>>();

    let is_chat_model = |m: &str| {
        let m = m.to_lowercase();
        // Known text-instruction / chat families.
        let chat_families = [
            "qwen", "llama", "mistral", "mixtral", "gemma", "phi", "command",
            "hermes", "neural", "solar", "openchat", "wizard", "vicuna", "tinyllama",
        ];
        let is_chat_family = chat_families.iter().any(|f| m.contains(f));
        // Reject vision and embedding models even if their name matches a family.
        let is_non_chat = [
            "moondream", "bge-", "nomic-embed", "all-minilm", "clip", "llava",
            "vision", "embed", "e5-", "gte-", "multilingual-e5",
        ]
        .iter()
        .any(|n| m.contains(n));
        is_chat_family && !is_non_chat
    };

    // Prefer models that explicitly advertise "instruct" or "chat" in their tag.
    for (idx, m) in lower.iter().enumerate() {
        if (m.contains("instruct") || m.contains("chat")) && is_chat_model(m) {
            return Some(models[idx].clone());
        }
    }

    // Fall back to any chat-capable model.
    for (idx, m) in lower.iter().enumerate() {
        if is_chat_model(m) {
            return Some(models[idx].clone());
        }
    }

    None
}

/// Return the explicitly configured LLM client, or build one on demand from the
/// latest discovered AI inference server.
async fn resolve_llm_client(state: &AppState) -> Option<LlmClient> {
    if let Some(llm) = &state.llm {
        return Some(llm.clone());
    }

    let servers = state
        .store
        .latest_metric()
        .await
        .ok()
        .flatten()
        .map(|m| m.ai_servers)
        .unwrap_or_default();

    for server in servers {
        if let Some(client) = llm_client_from_discovered_server(&server) {
            return Some(client);
        }
    }

    None
}

/// Send a prompt to the real LLM if configured (or auto-discovered), otherwise to the
/// fallback responder.
///
/// This is the low-level completion seam used by chat, one-off explanations, and the
/// background insight digest. The returned assistant message has any visible reasoning
/// chain stripped so callers receive only the final answer.
///
/// A `timeout` of `None` waits indefinitely for the real LLM. The fallback responder is
/// always synchronous, so the timeout only affects the network path.
pub async fn complete_with_fallback(
    state: &AppState,
    prompt: Vec<ChatMessage>,
    timeout: Option<Duration>,
) -> Result<ChatMessage> {
    if let Some(llm) = resolve_llm_client(state).await {
        let completion = llm.complete(&prompt);
        let result = match timeout {
            Some(limit) => tokio_timeout(limit, completion).await,
            None => Ok(completion.await),
        };
        let mut reply = match result {
            Ok(Ok(reply)) => reply,
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(ObservaError::Llm(
                    "llm completion timed out".to_string(),
                ))
            }
        };
        reply.content = strip_reasoning_chain(&reply.content);
        return Ok(reply);
    }

    if let Some(fallback) = &state.fallback {
        fallback.complete(&prompt).await
    } else {
        Err(ObservaError::Config("llm client is not configured".to_string()))
    }
}

/// Stream a chat completion from the real LLM if configured (or auto-discovered).
///
/// Returns `Ok(None)` when no LLM is available so the caller can fall back to the
/// synchronous rule-based responder.
pub async fn complete_stream_with_fallback(
    state: &AppState,
    prompt: &[ChatMessage],
    timeout: Duration,
) -> Result<Option<impl tokio_stream::Stream<Item = Result<String>> + Send + 'static>> {
    if let Some(llm) = resolve_llm_client(state).await {
        let stream = match tokio_timeout(timeout, llm.complete_stream(prompt)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(ObservaError::Llm("llm stream timed out".to_string())),
        };
        return Ok(Some(stream));
    }

    Ok(None)
}

/// Remove any chain-of-thought or reasoning prefix from model output.
///
/// Several models expose their internal thinking as a leading block marked with
/// "Here's a thinking process:" or `<think>`/`<thinking>` tags. This function
/// discards that block and returns only the final answer, or the original text
/// if no marker is found.
pub fn strip_reasoning_chain(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.starts_with("Here's a thinking process:")
        || trimmed.starts_with("Here's my thinking process:")
        || trimmed.starts_with("Thinking process:")
        || trimmed.to_lowercase().starts_with("<think>")
    {
        let mut lines = trimmed.lines().skip_while(|l| {
            let t = l.trim();
            !(t.starts_with(|c: char| c.is_ascii_digit()) && t.starts_with("**"))
        });
        let mut result: Vec<&str> = Vec::new();
        let mut found_final = false;
        for line in lines.by_ref() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if (t.starts_with("Final Answer")
                || t.starts_with("**Final Answer**")
                || t.starts_with("Answer:")
                || t.starts_with("**Answer:**")
                || t.eq_ignore_ascii_case("</think>"))
                && !found_final
            {
                found_final = true;
                continue;
            }
            if found_final {
                result.push(line);
            }
        }
        if found_final {
            return result.join("\n").trim().to_string();
        }
    }

    // Strip `<think>...</think>` blocks when the whole content is wrapped in them.
    let lower = trimmed.to_lowercase();
    if lower.starts_with("<think>") {
        if let Some(end) = lower.find("</think>") {
            return trimmed[end + 8..].trim().to_string();
        }
    }

    content.to_string()
}
