use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Query, State},

    response::{sse::Event as SseEvent, Html, IntoResponse, Sse},
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::unbounded_channel;
use tokio::time::timeout;
use tokio_stream::{wrappers::UnboundedReceiverStream, StreamExt};
use uuid::Uuid;

use observa_shared::{format_bytes, ChatMessage, Event, LogEvent, MetricSnapshot, ObservaError, Result, Role};

use crate::auth::{chat_owner_token_from_headers, owner_token_cookie};
use crate::llm::{complete_with_fallback, strip_reasoning_chain};
use crate::llm_sanitize::{clamp_prompt, format_log_sanitized, sanitize_for_prompt, validate_message_length};
use crate::rate_limit::{rate_limit_check, RateLimitConfig};
use crate::state::AppState;

const CHAT_RATE_LIMIT: RateLimitConfig = RateLimitConfig { max: 20, window: Duration::from_secs(60) };

fn chat_llm_timeout() -> Duration {
    std::env::var("OBSERVA_CHAT_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(120))
}

/// Request body for `POST /api/chat/session`.
#[derive(Debug, Deserialize)]
pub struct CreateSession {}

/// Response body for `POST /api/chat/session`.
#[derive(Debug, Serialize)]
pub struct SessionResponse {
    session_id: Uuid,
    owner_token: String,
}

/// Request body for `POST /api/chat/ask`.
#[derive(Debug, Deserialize)]
pub struct AskRequest {
    session_id: Uuid,
    owner_token: String,
    message: String,
    system_prompt: Option<String>,
}

impl AskRequest {
    /// Return the owner token, preferring the explicit body field but
    /// falling back to the `X-Owner-Token` header when present.
    fn owner_token(&self, header_token: Option<String>) -> String {
        header_token.filter(|t| !t.is_empty()).unwrap_or_else(|| self.owner_token.clone())
    }
}

/// Response body for `POST /api/chat/ask`.
#[derive(Debug, Serialize)]
pub struct AskResponse {
    reply: String,
}

/// Query params for `GET /api/chat/stream`.
#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    session_id: Uuid,
    owner_token: String,
    message: String,
    system_prompt: Option<String>,
}

pub async fn create_session(
    State(state): State<Arc<AppState>>,
    crate::rate_limit::ClientIp(addr): crate::rate_limit::ClientIp,
) -> axum::response::Response {
    if let Err(resp) = rate_limit_check(&state, "chat_session", addr, CHAT_RATE_LIMIT).await {
        return resp.into_response();
    }
    match state.chat_store.create_session().await {
        Ok((session_id, owner_token)) => {
            (
                [(
                    axum::http::header::SET_COOKIE,
                    axum::http::HeaderValue::from_str(&owner_token_cookie(&owner_token))
                        .unwrap_or_else(|_| axum::http::HeaderValue::from_static("")),
                )],
                Json(SessionResponse { session_id, owner_token }),
            )
                .into_response()
        }
        Err(err) => error_response(err),
    }
}

pub async fn ask(
    State(state): State<Arc<AppState>>,
    crate::rate_limit::ClientIp(addr): crate::rate_limit::ClientIp,
    headers: axum::http::HeaderMap,
    Json(req): Json<AskRequest>,
) -> axum::response::Response {
    if let Err(resp) = rate_limit_check(&state, "chat_ask", addr, CHAT_RATE_LIMIT).await {
        return resp.into_response();
    }
    if let Err(err) = validate_message_length(&req.message) {
        return error_response(err);
    }
    let owner_token = req.owner_token(chat_owner_token_from_headers(&headers));
    match ask_core(&state, req.session_id, &owner_token, &req.message, req.system_prompt.as_deref()).await {
        Ok(reply) => {
            if let Err(e) = state.bus.publish(Event::Chat(reply.clone())) {
                tracing::warn!(error = %e, "failed to publish chat event");
            }
            Json(AskResponse {
                reply: reply.content,
            })
            .into_response()
        }
        Err(err) => error_response(err),
    }
}

pub async fn ask_html(
    State(state): State<Arc<AppState>>,
    crate::rate_limit::ClientIp(addr): crate::rate_limit::ClientIp,
    headers: axum::http::HeaderMap,
    Json(req): Json<AskRequest>,
) -> axum::response::Response {
    if let Err(resp) = rate_limit_check(&state, "chat_ask_html", addr, CHAT_RATE_LIMIT).await {
        return resp.into_response();
    }
    if let Err(err) = validate_message_length(&req.message) {
        return render_chat_reply(&state, &format!("Error: {err}")).await;
    }
    let owner_token = req.owner_token(chat_owner_token_from_headers(&headers));
    match ask_core(&state, req.session_id, &owner_token, &req.message, req.system_prompt.as_deref()).await {
        Ok(reply) => render_chat_reply(&state, &reply.content).await,
        Err(err) => render_chat_reply(&state, &format!("Error: {err}")).await,
    }
}

async fn render_chat_reply(state: &AppState, reply: &str) -> axum::response::Response {
    let mut ctx = tera::Context::new();
    ctx.insert("reply", reply);
    match state.tera.render("partials/chat_reply.html", &ctx) {
        Ok(html) => Html(html).into_response(),
        Err(e) => error_response(ObservaError::Config(format!("template error: {e}"))),
    }
}

async fn ask_core(
    state: &AppState,
    session_id: Uuid,
    owner_token: &str,
    message: &str,
    system_prompt: Option<&str>,
) -> Result<ChatMessage> {
    ensure_session_owner(state, session_id, owner_token).await?;

    let mut messages = load_context(state, session_id).await;
    messages.push(ChatMessage {
        role: Role::User,
        content: sanitize_for_prompt(message),
    });
    let prompt = prepend_system_prompt(messages, system_prompt);
    let prompt = clamp_prompt(prompt);

    let reply = complete_with_fallback(state, prompt, Some(chat_llm_timeout())).await?;

    persist_messages(state, session_id, message, &reply).await?;
    Ok(reply)
}

pub async fn stream(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StreamQuery>,
    crate::rate_limit::ClientIp(addr): crate::rate_limit::ClientIp,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = rate_limit_check(&state, "chat_stream", addr, CHAT_RATE_LIMIT).await {
        return resp.into_response();
    }
    if let Err(err) = validate_message_length(&query.message) {
        return error_response(err);
    }
    let owner_token = chat_owner_token_from_headers(&headers)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| query.owner_token.clone());
    match do_stream(state, query.session_id, &owner_token, query.message, query.system_prompt).await {
        Ok(sse) => sse.into_response(),
        Err(err) => error_response(err),
    }
}

async fn do_stream(
    state: Arc<AppState>,
    session_id: Uuid,
    owner_token: &str,
    message: String,
    system_prompt: Option<String>,
) -> Result<
    Sse<
        std::pin::Pin<
            Box<
                dyn tokio_stream::Stream<Item = std::result::Result<SseEvent, std::convert::Infallible>>
                    + Send,
            >,
        >,
    >,
> {
    ensure_session_owner(&state, session_id, owner_token).await?;

    let mut messages = load_context(&state, session_id).await;
    messages.push(ChatMessage {
        role: Role::User,
        content: sanitize_for_prompt(&message),
    });
    let prompt = prepend_system_prompt(messages, system_prompt.as_deref());
    let prompt = clamp_prompt(prompt);

    state
        .chat_store
        .store_message(
            session_id,
            &ChatMessage {
                role: Role::User,
                content: message.clone(),
            },
        )
        .await?;

    if let Some(llm) = &state.llm {
        let stream = match timeout(chat_llm_timeout(), llm.complete_stream(&prompt)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(ObservaError::Llm(
                    "llm stream timed out".to_string(),
                ));
            }
        };

        let bus = state.bus.clone();
        let chat_store = state.chat_store.clone();
        let (tx, rx) = unbounded_channel();
        tokio::spawn(stream_llm_reply(stream, session_id, chat_store, bus, tx));

        return Ok(Sse::new(Box::pin(UnboundedReceiverStream::new(rx)) as _));
    }

    if let Some(fallback) = &state.fallback {
        let reply = fallback.complete(&prompt).await?;
        let content = reply.content.clone();
        if let Err(e) = state.bus.publish(Event::Chat(reply.clone())) {
            tracing::warn!(error = %e, "failed to publish fallback chat event");
        }
        let stream = tokio_stream::iter(vec![
            Ok(SseEvent::default().data(strip_reasoning_chain(&content))),
            Ok(SseEvent::default().event("done").data("")),
        ]);
        if let Err(e) = state.chat_store.store_message(session_id, &reply).await {
            tracing::warn!(error = %e, session_id = %session_id, "failed to persist fallback chat reply");
        }
        return Ok(Sse::new(Box::pin(stream) as _));
    }

    Err(ObservaError::Config("llm client is not configured".to_string()))
}

async fn stream_llm_reply<S>(
    stream: S,
    session_id: Uuid,
    chat_store: Arc<dyn crate::store::ChatStore>,
    bus: observa_bus::Bus,
    tx: tokio::sync::mpsc::UnboundedSender<std::result::Result<SseEvent, std::convert::Infallible>>,
) where
    S: tokio_stream::Stream<Item = Result<String>> + Unpin + Send + 'static,
{
    let mut content = String::new();
    let mut stream = stream;
    while let Some(result) = stream.next().await {
        match result {
            Ok(token) => {
                content.push_str(&token);
                if tx.send(Ok(SseEvent::default().data(token))).is_err() {
                    break;
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "llm stream token error");
                if tx.send(Ok(SseEvent::default().data(format!("[error: {e}]")))).is_err() {
                    break;
                }
            }
        }
    }
    if tx.send(Ok(SseEvent::default().event("done").data(""))).is_err() {
        tracing::debug!("sse receiver dropped before done event");
    }
    let full_reply = ChatMessage {
        role: Role::Assistant,
        content: strip_reasoning_chain(&content),
    };
    if let Err(e) = chat_store.store_message(session_id, &full_reply).await {
        tracing::warn!(error = %e, session_id = %session_id, "failed to persist streamed assistant reply");
    } else if let Err(e) = bus.publish(Event::Chat(full_reply)) {
        tracing::warn!(error = %e, "failed to publish streamed chat event");
    }
}

async fn ensure_session_owner(
    state: &AppState,
    session_id: Uuid,
    owner_token: &str,
) -> Result<()> {
    state.chat_store.ensure_session(session_id, owner_token).await?;
    match state.chat_store.verify_session_owner(session_id, owner_token).await? {
        true => Ok(()),
        false => Err(ObservaError::Store(
            "chat session owner token mismatch".to_string(),
        )),
    }
}

async fn load_context(state: &AppState, session_id: Uuid) -> Vec<ChatMessage> {
    let mut messages = Vec::new();

    if let Ok(history) = state.chat_store.messages_for_session(session_id).await {
        messages.extend(history);
    }

    if let Ok(Some(m)) = state.store.latest_metric().await {
        messages.push(format_metric(&m));
    }

    let logs = state.store.recent_logs(5).await.unwrap_or_default();
    messages.extend(logs.iter().map(format_log));

    messages
}

fn prepend_system_prompt(
    mut messages: Vec<ChatMessage>,
    system_prompt: Option<&str>,
) -> Vec<ChatMessage> {
    const DEFAULT_PROMPT: &str = "You are Observa, a friendly and helpful assistant. You can chat about anything the user likes, including casual or off-topic requests. When the user asks about this system, use the provided metrics and logs to give a concise, accurate answer. Keep responses brief unless asked for detail. Do not show your thinking process, chain-of-thought, or any internal analysis.";
    const MAX_PROMPT_LEN: usize = 4000;
    let content = system_prompt
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.len() > MAX_PROMPT_LEN {
                s[..MAX_PROMPT_LEN].to_string()
            } else {
                s.to_string()
            }
        })
        .unwrap_or_else(|| DEFAULT_PROMPT.to_string());
    messages.insert(
        0,
        ChatMessage {
            role: Role::System,
            content,
        },
    );
    messages
}


fn format_metric(m: &MetricSnapshot) -> ChatMessage {
    let top: Vec<String> = m
        .processes
        .iter()
        .take(5)
        .map(|p| format!("{} ({:.1}% CPU, {})", p.name, p.cpu_percent, format_bytes(p.memory_bytes)))
        .collect();
    let ai: Vec<String> = m
        .ai_servers
        .iter()
        .map(|a| format!("{} [{:?}]", a.name, a.kind))
        .collect();
    let mut content = format!(
        "Latest metrics: CPU {:.1}%, memory {}/{} bytes, {} disks, {} networks, {} processes. Top processes: {}.",
        m.cpu.usage_percent,
        m.memory.used_bytes,
        m.memory.total_bytes,
        m.disks.len(),
        m.networks.len(),
        m.processes.len(),
        top.join(", ")
    );
    if !ai.is_empty() {
        content.push_str(&format!(" AI servers: {}.", ai.join(", ")));
    }
    ChatMessage {
        role: Role::User,
        content,
    }
}

fn format_log(l: &LogEvent) -> ChatMessage {
    format_log_sanitized(l)
}

async fn persist_messages(
    state: &AppState,
    session_id: Uuid,
    user_message: &str,
    reply: &ChatMessage,
) -> Result<()> {
    state
        .chat_store
        .store_message(
            session_id,
            &ChatMessage {
                role: Role::User,
                content: user_message.to_string(),
            },
        )
        .await?;
    state.chat_store.store_message(session_id, reply).await?;
    Ok(())
}

fn error_response(err: ObservaError) -> axum::response::Response {
    let status = match err {
        ObservaError::Config(_) => axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        ObservaError::Llm(_) => axum::http::StatusCode::BAD_GATEWAY,
        _ => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
    };
    let body = Json(serde_json::json!({"error": err.to_string()}));
    (status, body).into_response()
}
