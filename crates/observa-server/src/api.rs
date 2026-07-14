use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use observa_shared::{ChatMessage, HealthStatus, InsightSnapshot, LogEvent, Role};
use crate::{chat, llm, state::AppState};
use crate::auth::OWNER_TOKEN_HEADER;
use crate::llm_sanitize::{clamp_prompt, format_log_for_explanation, validate_message_length};
use crate::rate_limit::{rate_limit_check, ClientIp, RateLimitConfig, API_READ_RATE_LIMIT};

fn store_error_response(err: observa_shared::ObservaError) -> Response {
    tracing::warn!(error = %err, "store read failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": err.to_string() })),
    )
        .into_response()
}

/// Build the `/api/*` router with the same state type as the dashboard router.
pub fn api_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/insights", get(insights))
        .route("/metrics/history", get(metrics_history))
        .route("/metrics/latest", get(metrics_latest))
        .route("/logs/history", get(logs_history))
        .route("/logs/explain", post(explain_log))
        .route("/alerts/acknowledge", post(acknowledge_alert))
        .route("/alerts/acknowledged", get(list_acknowledged_alerts))
        .route("/alerts/verify-chain", get(verify_alert_chain))
        .route("/alerts/export", get(export_alerts))
        .route("/chat/session", post(chat::create_session))
        .route("/chat/ask", post(chat::ask))
        .route("/chat/ask-html", post(chat::ask_html))
        .route("/chat/stream", get(chat::stream))
        .with_state(state)
}

#[derive(Debug, serde::Serialize)]
struct HealthResponse {
    status: HealthStatus,
    ok: bool,
}

async fn health(State(state): State<Arc<AppState>>) -> Response {
    let health = state.background.health().await;
    let (status, ok) = match health {
        HealthStatus::Healthy => (StatusCode::OK, true),
        HealthStatus::Degraded => (StatusCode::OK, false),
        HealthStatus::Unhealthy => (StatusCode::SERVICE_UNAVAILABLE, false),
    };
    (status, Json(HealthResponse { status: health, ok })).into_response()
}

#[derive(Debug, serde::Serialize)]
struct StatusResponse {
    health: HealthStatus,
    heartbeat_seq: u64,
    insight: Option<InsightSnapshot>,
    llm_ok: bool,
    retention_days: u64,
    stored_metrics: usize,
    stored_logs: usize,
    compression_enabled: bool,
}

async fn status(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "api_status", addr, API_READ_RATE_LIMIT).await {
        return resp.into_response();
    }
    let (stored_metrics, stored_logs) = match state.store.store_counts().await {
        Ok(counts) => counts,
        Err(err) => return store_error_response(err),
    };
    Json(StatusResponse {
        health: state.background.health().await,
        heartbeat_seq: state.background.next_heartbeat_seq() - 1,
        insight: state.background.insight().await,
        llm_ok: state.llm.is_some(),
        retention_days: state.config.retention_days,
        stored_metrics,
        stored_logs,
        compression_enabled: state.config.compression_enabled,
    })
    .into_response()
}

#[derive(Debug, serde::Serialize)]
struct InsightsResponse {
    insight: Option<InsightSnapshot>,
}

async fn insights(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "api_insights", addr, API_READ_RATE_LIMIT).await {
        return resp.into_response();
    }
    Json(InsightsResponse {
        insight: state.background.insight().await,
    })
    .into_response()
}

async fn metrics_history(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(
        &state, "api_metrics_history", addr, API_READ_RATE_LIMIT).await {
        return resp.into_response();
    }
    match state.store.recent_metrics(100).await {
        Ok(metrics) => Json(metrics).into_response(),
        Err(err) => store_error_response(err),
    }
}

async fn metrics_latest(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(
        &state, "api_metrics_latest", addr, API_READ_RATE_LIMIT).await {
        return resp.into_response();
    }
    match state.store.latest_metric().await {
        Ok(metric) => Json(metric).into_response(),
        Err(err) => store_error_response(err),
    }
}

async fn logs_history(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(
        &state, "api_logs_history", addr, API_READ_RATE_LIMIT).await {
        return resp.into_response();
    }
    match state.store.recent_logs(100).await {
        Ok(logs) => Json(logs).into_response(),
        Err(err) => store_error_response(err),
    }
}

#[derive(Debug, serde::Deserialize)]
struct ExplainLogRequest {
    log: LogEvent,
}

fn sha256_prefix(input: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Debug, serde::Serialize)]
struct ExplainLogResponse {
    explanation: String,
}

const EXPLAIN_RATE_LIMIT: RateLimitConfig = RateLimitConfig { max: 10, window: Duration::from_secs(60) };
const ACKNOWLEDGE_RATE_LIMIT: RateLimitConfig = RateLimitConfig { max: 30, window: Duration::from_secs(60) };

async fn explain_log(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
    Json(req): Json<ExplainLogRequest>,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "explain", addr, EXPLAIN_RATE_LIMIT).await {
        return resp.into_response();
    }
    if let Err(err) = validate_message_length(&req.log.message) {
        return (StatusCode::PAYLOAD_TOO_LARGE, Json(serde_json::json!({"error": err.to_string()}))).into_response();
    }

    let message_key = req.log.message.clone();
    if let Some(cached) = state.background.explanation(&message_key).await {
        return Json(ExplainLogResponse { explanation: cached }).into_response();
    }

    let prompt = vec![
        ChatMessage {
            role: Role::System,
            content: "You are Observa, a system operations assistant. Explain the given log event in one or two sentences: what likely happened, and what action a human operator should consider, if any. Be terse.".to_string(),
        },
        ChatMessage {
            role: Role::User,
            content: format_log_for_explanation(&req.log),
        },
    ];
    let prompt = clamp_prompt(prompt);

    match llm::complete_with_fallback(&state, prompt, None).await {
        Ok(reply) => {
            let clean = llm::strip_reasoning_chain(&reply.content);
            state
                .background
                .set_explanation(message_key, clean.clone())
                .await;
            Json(ExplainLogResponse { explanation: clean }).into_response()
        }
        Err(err) => {
            let status = match err {
                observa_shared::ObservaError::Config(_) => StatusCode::UNPROCESSABLE_ENTITY,
                observa_shared::ObservaError::Llm(_) => StatusCode::BAD_GATEWAY,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            let body = Json(ExplainLogErrorResponse { error: err.to_string() });
            (status, body).into_response()
        }
    }
}

#[derive(Debug, serde::Serialize)]
struct ExplainLogErrorResponse {
    error: String,
}



#[derive(Debug, serde::Deserialize)]
struct AcknowledgeAlertRequest {
    key: String,
}

#[derive(Debug, serde::Serialize)]
struct AcknowledgeAlertResponse {
    acknowledged: Vec<String>,
}

async fn acknowledge_alert(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
    headers: axum::http::HeaderMap,
    Json(req): Json<AcknowledgeAlertRequest>,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "acknowledge", addr, ACKNOWLEDGE_RATE_LIMIT).await {
        return resp.into_response();
    }
    let actor = headers
        .get(OWNER_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_default();
    tracing::info!(key = %req.key, actor = %sha256_prefix(&actor), "alert acknowledged");
    state.background.acknowledge_alert(req.key).await;
    Json(AcknowledgeAlertResponse {
        acknowledged: state.background.list_acknowledged_alerts().await,
    })
    .into_response()
}

async fn list_acknowledged_alerts(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(
        &state, "api_alerts_acknowledged", addr, API_READ_RATE_LIMIT).await {
        return resp.into_response();
    }
    Json(AcknowledgeAlertResponse {
        acknowledged: state.background.list_acknowledged_alerts().await,
    })
    .into_response()
}

#[derive(Debug, serde::Serialize)]
struct VerifyAlertChainResponse {
    ok: bool,
    checked: usize,
    broken: Vec<String>,
}

async fn verify_alert_chain(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(
        &state, "api_alerts_verify_chain", addr, API_READ_RATE_LIMIT).await {
        return resp.into_response();
    }
    let Some(db) = state.db.as_ref() else {
        return Json(VerifyAlertChainResponse { ok: true, checked: 0, broken: Vec::new() }).into_response();
    };
    match observa_db::security::verify_chain(db).await {
        Ok(broken) => Json(VerifyAlertChainResponse {
            ok: broken.is_empty(),
            checked: broken.len(),
            broken,
        }).into_response(),
        Err(err) => store_error_response(err),
    }
}

async fn export_alerts(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(
        &state, "api_alerts_export", addr, API_READ_RATE_LIMIT).await {
        return resp.into_response();
    }
    let alerts = match state.store.security_alerts(usize::MAX).await {
        Ok(alerts) => alerts,
        Err(err) => return store_error_response(err),
    };
    let filename = format!("observa-alerts-{}.json", chrono::Utc::now().format("%Y%m%dT%H%M%SZ"));
    let body = serde_json::to_string_pretty(&alerts).unwrap_or_else(|_| "[]".to_string());
    (
        StatusCode::OK,
        [
            ("Content-Type", "application/json"),
            ("Content-Disposition", &format!("attachment; filename=\"{}\"", filename)),
        ],
        body,
    )
        .into_response()
}
