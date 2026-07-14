use std::time::Duration;

use observa_shared::{ChatMessage, ObservaError, Result};
use tokio::time::timeout as tokio_timeout;

use crate::state::AppState;

/// Send a prompt to the real LLM if configured, otherwise to the fallback responder.
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
    if let Some(llm) = &state.llm {
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
        Ok(reply)
    } else if let Some(fallback) = &state.fallback {
        fallback.complete(&prompt).await
    } else {
        Err(ObservaError::Config("llm client is not configured".to_string()))
    }
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
