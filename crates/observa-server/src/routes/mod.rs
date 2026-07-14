use std::error::Error as StdError;
use std::sync::Arc;

use axum::{
    extract::{OriginalUri, Query, State},
    http::header,
    middleware,
    response::{Html, IntoResponse, Response},
    routing::{get, get_service, post},
    Router,
};
use tower_http::{
    limit::RequestBodyLimitLayer,
    services::ServeDir,
};
use uuid::Uuid;

use observa_bus::sse_stream;
use observa_shared::severity_class;

use crate::api::api_routes;
use crate::auth::{auth_middleware, chat_owner_token, dashboard_token_cookie, owner_token_cookie};
use crate::paths::workspace_root;
use crate::rate_limit::{
    global_rate_limit_middleware, rate_limit_check, ClientIp, HTML_RATE_LIMIT,
};
use crate::routes::params::{
    filtered_logs, filtered_security_alerts, parse_severity_filter, ChatQuery, LogFilter,
    MetricRange, SecurityFilter, SeverityCount,
};
use crate::routes::view::{
    AiServerCard, DiskCard, LogRow, MetricSummary, NetworkCard, NetworkCombined, NetworkTrafficRow,
    ProcessCard, ProcessEventRow, SecurityAlertRow, StorageEventRow, SwapCard,
};
use crate::state::AppState;

pub mod params;
pub mod view;

/// Maximum allowed request body size for chat and API endpoints (1 MiB).
const MAX_BODY_SIZE: usize = 1024 * 1024;

/// Build the full dashboard router.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/metrics", get(metrics_page))
        .route("/logs", get(logs_page))
        .route("/security", get(security_page))
        .route("/network", get(network_page))
        .route("/processes", get(processes_page))
        .route("/storage", get(storage_page))
        .route("/ai-servers", get(ai_servers_page))
        .route("/status", get(status_page))
        .route("/chat", get(chat_page))
        .route("/login", get(login_page))
        .route("/login", post(login_submit))
        .route("/logout", post(logout))
        .route("/about", get(about_page))
        .route("/events", get(events))
        .route("/partials/metrics", get(partial_metrics))
        .route("/partials/metrics-summary", get(partial_metrics_summary))
        .route("/partials/logs", get(partial_logs))
        .route("/partials/security", get(partial_security))
        .route("/partials/network", get(partial_network))
        .route("/partials/processes", get(partial_processes))
        .route("/partials/storage", get(partial_storage))
        .route("/partials/ai-servers", get(partial_ai_servers))
        .route("/partials/chat", get(partial_chat))
        .nest("/api", api_routes(state.clone()))
        .nest_service("/assets", get_service(ServeDir::new(assets_dir())))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_SIZE))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
        .layer(middleware::from_fn_with_state(state.clone(), global_rate_limit_middleware))
        .layer(middleware::from_fn(security_headers_middleware))
        .with_state(state)
}

fn assets_dir() -> std::path::PathBuf {
    workspace_root().join("assets")
}

/// Add baseline security headers to every response.
async fn security_headers_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_FRAME_OPTIONS,
        header::HeaderValue::from_static("DENY"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        header::HeaderName::from_static("content-security-policy"),
        header::HeaderValue::from_static(
            "default-src 'self'; script-src 'self' 'unsafe-inline' https://unpkg.com; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; font-src 'self' https://fonts.gstatic.com; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'self'; form-action 'self';",
        ),
    );
    response
}

/// Render a Tera template or return a plain 500 error page.
fn render(state: &AppState, template: &str, ctx: &tera::Context) -> impl IntoResponse {
    match state.tera.render(template, ctx) {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            let mut chain = String::new();
            chain.push_str(&format!("{e}"));
            let mut source = e.source();
            while let Some(s) = source {
                chain.push_str(&format!("\n  caused by: {s}"));
                source = s.source();
            }
            tracing::error!(error = %chain, template, "failed to render template");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to render {template}: {chain}"),
            )
                .into_response()
        }
    }
}

fn insert_path(ctx: &mut tera::Context, path: &str) {
    ctx.insert("request_path", path);
}

/// Degrade a store read to an empty/default value while logging the error.
async fn degrade_on_error<T: Default>(
    source: &str,
    result: observa_shared::Result<T>,
) -> T {
    match result {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(error = %err, "failed to read {} from store; degrading to empty", source);
            T::default()
        }
    }
}

macro_rules! simple_page {
    ($name:ident, $builder:path, $template:expr, $endpoint:expr) => {
        async fn $name(
            State(state): State<Arc<AppState>>,
            OriginalUri(uri): OriginalUri,
            ClientIp(addr): ClientIp,
        ) -> axum::response::Response {
            if let Err(resp) = rate_limit_check(&state, $endpoint, addr, HTML_RATE_LIMIT).await {
                return resp.into_response();
            }
            let mut ctx = $builder(&state).await;
            insert_path(&mut ctx, uri.path());
            render(&state, $template, &ctx).into_response()
        }
    };
}

macro_rules! simple_partial {
    ($name:ident, $builder:path, $template:expr, $endpoint:expr) => {
        async fn $name(
            State(state): State<Arc<AppState>>,
            OriginalUri(uri): OriginalUri,
            ClientIp(addr): ClientIp,
        ) -> axum::response::Response {
            if let Err(resp) = rate_limit_check(&state, $endpoint, addr, HTML_RATE_LIMIT).await {
                return resp.into_response();
            }
            let mut ctx = $builder(&state).await;
            insert_path(&mut ctx, uri.path());
            render(&state, $template, &ctx).into_response()
        }
    };
}

async fn index(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "index", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let mut ctx = build_dashboard_context(&state).await;
    let logs: Vec<LogRow> = filtered_logs(&state, &LogFilter::default(), 5)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to load recent logs for dashboard");
            Vec::new()
        })
        .iter()
        .map(LogRow::from_event)
        .collect();
    let security_ctx =
        build_security_context_with_limit(&state, &SecurityFilter::default(), 5).await;
    ctx.extend(security_ctx);
    let retention = build_retention_context(&state).await;
    ctx.extend(retention);
    ctx.insert("logs", &logs);
    insert_path(&mut ctx, uri.path());
    render(&state, "index.html", &ctx).into_response()
}

async fn build_dashboard_context(state: &AppState) -> tera::Context {
    let mut ctx = build_metrics_context(state, state.config.metric_history_minutes).await;
    let latest = state.store.latest_metric().await.unwrap_or_default();
    let processes: Vec<ProcessCard> = latest
        .as_ref()
        .map(|m| {
            m.processes
                .iter()
                .take(6)
                .map(ProcessCard::from_process)
                .collect()
        })
        .unwrap_or_default();
    let networks: Vec<NetworkCard> = latest
        .as_ref()
        .map(|m| {
            m.networks
                .iter()
                .take(4)
                .map(NetworkCard::from_network)
                .collect()
        })
        .unwrap_or_default();
    let disks: Vec<DiskCard> = latest
        .as_ref()
        .map(|m| {
            m.disks
                .iter()
                .take(4)
                .map(DiskCard::from_disk)
                .collect()
        })
        .unwrap_or_default();
    let network_combined = latest
        .as_ref()
        .map(|m| NetworkCombined::from_networks(&m.networks))
        .unwrap_or_else(|| NetworkCombined::from_networks(&[]));
    ctx.insert("processes", &processes);
    ctx.insert("process_count", &processes.len());
    ctx.insert("networks", &networks);
    ctx.insert("network_count", &networks.len());
    ctx.insert("network_combined", &network_combined);
    ctx.insert("disks", &disks);
    ctx.insert("disk_count", &disks.len());
    ctx
}

async fn metrics_page(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Query(range): Query<MetricRange>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "metrics_page", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let mut ctx = build_metrics_context(&state, range.minutes()).await;
    let dashboard_ctx = build_dashboard_context(&state).await;
    ctx.extend(dashboard_ctx);
    let retention = build_retention_context(&state).await;
    ctx.extend(retention);
    ctx.insert("range_minutes", &range.minutes());
    insert_path(&mut ctx, uri.path());
    render(&state, "metrics.html", &ctx).into_response()
}

async fn logs_page(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Query(filter): Query<LogFilter>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "logs_page", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let mut ctx = build_logs_context(&state, &filter).await;
    insert_path(&mut ctx, uri.path());
    render(&state, "logs.html", &ctx).into_response()
}

async fn chat_page(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    ClientIp(addr): ClientIp,
    Query(query): Query<ChatQuery>,
    request: axum::extract::Request,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "chat_page", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let owner_token = chat_owner_token(&request).or(query.owner_token.clone()).unwrap_or_default();
    let mut ctx = build_chat_context(&state, query.session_id, &owner_token).await;
    // build_chat_context inserts owner_token only when it creates a new session;
    // make sure the queried token is available to the template as well.
    let token_for_cookie = ctx
        .get("owner_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or(owner_token);
    if !ctx.contains_key("owner_token") {
        ctx.insert("owner_token", &token_for_cookie);
    }
    insert_path(&mut ctx, uri.path());
    let html = render(&state, "chat.html", &ctx);
    (
        [(
            header::SET_COOKIE,
            header::HeaderValue::from_str(&owner_token_cookie(&token_for_cookie))
                .unwrap_or_else(|_| header::HeaderValue::from_static("")),
        )],
        html,
    )
        .into_response()
}

async fn login_page(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "login_page", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let mut ctx = tera::Context::new();
    insert_path(&mut ctx, uri.path());
    render(&state, "login.html", &ctx).into_response()
}

#[derive(Debug, serde::Deserialize)]
struct LoginForm {
    token: String,
}

async fn login_submit(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
    axum::Form(form): axum::Form<LoginForm>,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "login_submit", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let expected = match &state.config.dashboard_token {
        Some(t) => t,
        None => {
            return axum::response::Redirect::to("/").into_response();
        }
    };
    let trimmed = form.token.trim();
    if !crate::auth::constant_time_eq(expected, trimmed) {
        tracing::warn!(addr = %addr, "failed login attempt");
        let mut ctx = tera::Context::new();
        ctx.insert("error", "Invalid token");
        ctx.insert("request_path", "/login");
        return render(&state, "login.html", &ctx).into_response();
    }
    (
        [(
            header::SET_COOKIE,
            header::HeaderValue::from_str(&dashboard_token_cookie(trimmed))
                .unwrap_or_else(|_| header::HeaderValue::from_static("")),
        )],
        axum::response::Redirect::to("/"),
    )
        .into_response()
}

async fn logout(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "logout", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    (
        [(
            header::SET_COOKIE,
            header::HeaderValue::from_static(
                "observa_dashboard_token=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
            ),
        )],
        axum::response::Redirect::to("/login"),
    )
        .into_response()
}

async fn about_page(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "about_page", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let mut ctx = build_about_context(&state).await;
    insert_path(&mut ctx, uri.path());
    render(&state, "about.html", &ctx).into_response()
}

async fn build_about_context(state: &AppState) -> tera::Context {
    let mut ctx = tera::Context::new();
    ctx.insert("version", env!("CARGO_PKG_VERSION"));
    ctx.insert("sample_interval_ms", &state.config.sample_interval_ms);
    ctx.insert("log_source", &format!("{:?}", state.config.log_source));
    ctx.insert("retention_days", &state.config.retention_days);
    ctx.insert("compression_enabled", &state.config.compression_enabled);
    ctx.insert("database_enabled", &state.config.database_url.is_some());
    ctx.insert("redis_enabled", &state.config.redis_url.is_some());
    ctx
}

async fn build_status_context(state: &AppState) -> tera::Context {
    let mut ctx = tera::Context::new();
    ctx.insert("health", &state.background.health().await);
    ctx.insert("heartbeat_seq", &state.background.next_heartbeat_seq().saturating_sub(1));
    ctx.insert("insight", &state.background.insight().await);
    let (stored_metrics, stored_logs) = degrade_on_error("store_counts", state.store.store_counts().await).await;
    ctx.insert("stored_metrics", &stored_metrics);
    ctx.insert("stored_logs", &stored_logs);
    ctx.insert("retention_days", &state.config.retention_days);
    ctx.insert("llm_ok", &state.llm.is_some());
    ctx.insert("compression_enabled", &state.config.compression_enabled);
    ctx.insert("sample_interval_ms", &state.config.sample_interval_ms);
    ctx.insert("log_source", &format!("{:?}", state.config.log_source));
    ctx.insert("database_enabled", &state.config.database_url.is_some());
    ctx.insert("redis_enabled", &state.config.redis_url.is_some());
    ctx.insert("version", env!("CARGO_PKG_VERSION"));
    let last_snapshot = degrade_on_error("latest_metric", state.store.latest_metric().await).await;
    ctx.insert("last_snapshot_at", &last_snapshot.map(|m| m.ts.to_rfc3339()).unwrap_or_default());
    ctx
}

simple_page!(network_page, build_network_context, "network.html", "network_page");
simple_page!(processes_page, build_processes_context, "processes.html", "processes_page");
simple_page!(storage_page, build_storage_context, "storage.html", "storage_page");
simple_page!(ai_servers_page, build_ai_servers_context, "ai_servers.html", "ai_servers_page");
simple_page!(status_page, build_status_context, "status.html", "status_page");

simple_partial!(partial_network, build_network_context, "partials/network_cards.html", "partial_network");
simple_partial!(partial_processes, build_processes_context, "partials/process_cards.html", "partial_processes");
simple_partial!(partial_storage, build_storage_context, "partials/storage_cards.html", "partial_storage");
simple_partial!(partial_ai_servers, build_ai_servers_context, "partials/ai_server_cards.html", "partial_ai_servers");

async fn security_page(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Query(filter): Query<SecurityFilter>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "security_page", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let mut ctx = build_security_context(&state, &filter).await;
    insert_path(&mut ctx, uri.path());
    ctx.insert("filter_severities", &filter.severity);
    render(&state, "security.html", &ctx).into_response()
}

async fn partial_security(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Query(filter): Query<SecurityFilter>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "partial_security", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let mut ctx = build_security_context(&state, &filter).await;
    insert_path(&mut ctx, uri.path());
    ctx.insert("filter_severities", &filter.severity);
    render(&state, "partials/security_rows.html", &ctx).into_response()
}

async fn partial_metrics(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Query(range): Query<MetricRange>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "partial_metrics", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let mut ctx = build_metrics_context(&state, range.minutes()).await;
    insert_path(&mut ctx, uri.path());
    render(&state, "partials/metrics_table.html", &ctx).into_response()
}

async fn partial_metrics_summary(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Query(range): Query<MetricRange>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(
        &state, "partial_metrics_summary", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let mut ctx = build_metrics_context(&state, range.minutes()).await;
    insert_path(&mut ctx, uri.path());
    render(&state, "partials/metrics_summary.html", &ctx).into_response()
}

async fn partial_logs(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Query(filter): Query<LogFilter>,
    ClientIp(addr): ClientIp,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "partial_logs", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let mut ctx = build_logs_context(&state, &filter).await;
    insert_path(&mut ctx, uri.path());
    render(&state, "partials/log_rows.html", &ctx).into_response()
}

async fn partial_chat(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<ChatQuery>,
    ClientIp(addr): ClientIp,
    request: axum::extract::Request,
) -> Response {
    if let Err(resp) = rate_limit_check(&state, "partial_chat", addr, HTML_RATE_LIMIT).await {
        return resp.into_response();
    }
    let owner_token = chat_owner_token(&request).or(query.owner_token.clone()).unwrap_or_default();
    let mut ctx = build_chat_context(&state, query.session_id, &owner_token).await;
    insert_path(&mut ctx, uri.path());
    render(&state, "partials/chat_messages.html", &ctx).into_response()
}

async fn build_metrics_context(state: &AppState, minutes: u64) -> tera::Context {
    let mut ctx = tera::Context::new();
    let history = degrade_on_error("recent_metrics_within", state.store.recent_metrics_within(minutes).await).await;
    let latest = if let Some(first) = history.first().cloned() {
        Some(first)
    } else {
        degrade_on_error("latest_metric", state.store.latest_metric().await).await
    };
    let summary = latest.as_ref().map(MetricSummary::from_snapshot);
    let history_json = serde_json::to_string(&history).unwrap_or_else(|_| "[]".to_string());
    let history_json_b64 = base64::Engine::encode(&base64::prelude::BASE64_STANDARD, &history_json);
    ctx.insert("summary", &summary);
    ctx.insert("metric_history", &history);
    ctx.insert("metric_history_json", &history_json);
    ctx.insert("metric_history_json_b64", &history_json_b64);
    ctx.insert("metric_history_minutes", &minutes);
    ctx
}

async fn build_retention_context(state: &AppState) -> tera::Context {
    let mut ctx = tera::Context::new();
    let (metric_count, log_count) = degrade_on_error("store_counts", state.store.store_counts().await).await;
    ctx.insert("retention_days", &state.config.retention_days);
    ctx.insert("compression_enabled", &state.config.compression_enabled);
    ctx.insert("metric_count", &metric_count);
    ctx.insert("log_count", &log_count);
    ctx
}

async fn build_logs_context(state: &AppState, filter: &LogFilter) -> tera::Context {
    let mut ctx = tera::Context::new();
    let page_size = if filter.page_size > 0 {
        filter.page_size
    } else {
        state.config.log_page_size as usize
    };
    let offset = filter.page.saturating_mul(page_size);
    let severities = parse_severity_filter(&filter.severity);
    let (logs, total) = degrade_on_error(
        "search_logs_paginated",
        state.store.search_logs_paginated(filter.q.as_deref(), &severities, offset, page_size).await,
    )
    .await;
    let logs: Vec<LogRow> = logs.iter().map(LogRow::from_event).collect();
    let total_pages = total.max(1).div_ceil(page_size);
    let q = filter.q.as_deref().unwrap_or("");
    let q_truncated = if q.len() > params::MAX_LOG_QUERY_LEN { &q[..params::MAX_LOG_QUERY_LEN] } else { q };
    ctx.insert("logs", &logs);
    ctx.insert("filter_q", &q_truncated);
    ctx.insert("filter_severities", &filter.severity);
    ctx.insert("page", &filter.page);
    ctx.insert("page_size", &page_size);
    ctx.insert("total", &total);
    ctx.insert("total_pages", &total_pages);
    ctx.insert("has_prev", &(filter.page > 0));
    ctx.insert("has_next", &((filter.page + 1) < total_pages));
    ctx
}

async fn build_chat_context(
    state: &AppState,
    session_id: Option<Uuid>,
    owner_token: &str,
) -> tera::Context {
    let mut ctx = tera::Context::new();
    let session_id = match session_id {
        Some(id) => match state.chat_store.verify_session_owner(id, owner_token).await {
            Ok(true) => id,
            Ok(false) => {
                tracing::warn!(%id, "chat owner token mismatch; creating new session");
                match state.chat_store.create_session().await {
                    Ok((new_id, token)) => {
                        ctx.insert("owner_token", &token);
                        new_id
                    }
                    Err(error) => {
                        tracing::warn!(%error, "failed to create chat session");
                        return ctx;
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, %id, "failed to verify chat session owner");
                return ctx;
            }
        },
        None => {
            // If the browser already has an owner token (e.g. from a previous
            // chat), restore that session instead of creating a new one so
            // history persists across page navigations.
            let restored = if !owner_token.is_empty() {
                match state.chat_store.session_by_owner_token(owner_token).await {
                    Ok(Some(id)) => Some(id),
                    Ok(None) => None,
                    Err(error) => {
                        tracing::warn!(%error, "failed to look up chat session by owner token");
                        None
                    }
                }
            } else {
                None
            };
            match restored {
                Some(id) => id,
                None => match state.chat_store.create_session().await {
                    Ok((id, token)) => {
                        ctx.insert("owner_token", &token);
                        id
                    }
                    Err(error) => {
                        tracing::warn!(%error, "failed to create chat session");
                        return ctx;
                    }
                },
            }
        }
    };
    let messages = state
        .chat_store
        .messages_for_session(session_id)
        .await
        .unwrap_or_default();
    ctx.insert("session_id", &session_id.to_string());
    ctx.insert("messages", &messages);
    ctx
}

async fn build_security_context(state: &AppState, filter: &SecurityFilter) -> tera::Context {
    build_security_context_with_limit(state, filter, 50).await
}

async fn build_security_context_with_limit(
    state: &AppState,
    filter: &SecurityFilter,
    limit: usize,
) -> tera::Context {
    let mut ctx = tera::Context::new();
    let severities = parse_severity_filter(&filter.severity);
    let alerts: Vec<SecurityAlertRow> = filtered_security_alerts(state, &severities, limit)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to load security alerts");
            Vec::new()
        })
        .iter()
        .map(SecurityAlertRow::from_alert)
        .collect();
    let summary = severity_counts(&alerts);
    ctx.insert("alerts", &alerts);
    ctx.insert("alert_count", &alerts.len());
    ctx.insert("severity_summary", &summary);
    ctx
}

async fn build_network_context(state: &AppState) -> tera::Context {
    let mut ctx = tera::Context::new();
    let latest = degrade_on_error("latest_metric", state.store.latest_metric().await).await;
    let networks: Vec<NetworkCard> = latest
        .as_ref()
        .map(|m| m.networks.iter().map(NetworkCard::from_network).collect())
        .unwrap_or_default();
    let combined = latest
        .as_ref()
        .map(|m| NetworkCombined::from_networks(&m.networks))
        .unwrap_or_else(|| NetworkCombined::from_networks(&[]));
    let ports: Vec<crate::ports::PortRow> = crate::ports::open_ports().await.unwrap_or_default();
    let traffic: Vec<NetworkTrafficRow> = degrade_on_error("recent_logs", state.store.recent_logs(20).await)
        .await
        .into_iter()
        .map(NetworkTrafficRow::from_log)
        .collect();
    ctx.insert("networks", &networks);
    ctx.insert("network_combined", &combined);
    ctx.insert("ports", &ports);
    ctx.insert("traffic", &traffic);
    ctx.insert("traffic_count", &traffic.len());
    ctx
}

async fn build_ai_servers_context(state: &AppState) -> tera::Context {
    let mut ctx = tera::Context::new();
    let latest = degrade_on_error("latest_metric", state.store.latest_metric().await).await;
    let servers: Vec<AiServerCard> = latest
        .map(|m| m.ai_servers.iter().map(AiServerCard::from_ai_server).collect())
        .unwrap_or_default();
    let events: Vec<StorageEventRow> = degrade_on_error("recent_logs", state.store.recent_logs(20).await)
        .await
        .into_iter()
        .map(StorageEventRow::from_log)
        .collect();
    ctx.insert("servers", &servers);
    ctx.insert("server_count", &servers.len());
    ctx.insert("ai_server_events", &events);
    ctx.insert("ai_server_event_count", &events.len());
    ctx
}

async fn build_processes_context(state: &AppState) -> tera::Context {
    let mut ctx = tera::Context::new();
    let latest = degrade_on_error("latest_metric", state.store.latest_metric().await).await;
    let processes: Vec<ProcessCard> = latest
        .map(|m| m.processes.iter().map(ProcessCard::from_process).collect())
        .unwrap_or_default();
    let events: Vec<ProcessEventRow> = degrade_on_error("recent_logs", state.store.recent_logs(20).await)
        .await
        .into_iter()
        .map(ProcessEventRow::from_log)
        .collect();
    ctx.insert("processes", &processes);
    ctx.insert("process_count", &processes.len());
    ctx.insert("process_events", &events);
    ctx.insert("process_event_count", &events.len());
    ctx
}

async fn build_storage_context(state: &AppState) -> tera::Context {
    let mut ctx = tera::Context::new();
    let latest = degrade_on_error("latest_metric", state.store.latest_metric().await).await;
    let disks: Vec<DiskCard> = latest
        .as_ref()
        .map(|m| m.disks.iter().map(DiskCard::from_disk).collect())
        .unwrap_or_default();
    let swap = latest.as_ref().and_then(SwapCard::from_snapshot);
    let events: Vec<StorageEventRow> = degrade_on_error("recent_logs", state.store.recent_logs(20).await)
        .await
        .into_iter()
        .map(StorageEventRow::from_log)
        .collect();
    ctx.insert("disks", &disks);
    ctx.insert("disk_count", &disks.len());
    if let Some(s) = &swap {
        ctx.insert("swap", &s);
    }
    ctx.insert("has_swap", &swap.is_some());
    ctx.insert("storage_events", &events);
    ctx.insert("storage_event_count", &events.len());
    ctx
}

trait SeverityLabel {
    fn severity(&self) -> &str;
}

impl SeverityLabel for LogRow {
    fn severity(&self) -> &str {
        &self.severity
    }
}

impl SeverityLabel for SecurityAlertRow {
    fn severity(&self) -> &str {
        &self.severity
    }
}

fn severity_counts(rows: &[impl SeverityLabel]) -> Vec<SeverityCount> {
    use observa_shared::Severity;
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for row in rows {
        *counts.entry(row.severity().to_string()).or_insert(0) += 1;
    }
    let mut out = Vec::new();
    for (label, severity) in [
        ("critical", Severity::Critical),
        ("error", Severity::Error),
        ("warn", Severity::Warn),
        ("info", Severity::Info),
        ("debug", Severity::Debug),
    ] {
        let key = format!("{:?}", severity);
        if let Some(&count) = counts.get(&key) {
            out.push(SeverityCount {
                severity: label.to_string(),
                class: severity_class(severity).to_string(),
                count,
            });
        }
    }
    out
}

async fn events(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    sse_stream(&state.bus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_range_minutes_maps_known_values() {
        assert_eq!(MetricRange { range: "15m".to_string() }.minutes(), 15);
        assert_eq!(MetricRange { range: "1h".to_string() }.minutes(), 60);
        assert_eq!(MetricRange { range: "6h".to_string() }.minutes(), 360);
        assert_eq!(MetricRange { range: "24h".to_string() }.minutes(), 1440);
        assert_eq!(MetricRange { range: "7d".to_string() }.minutes(), 10080);
    }

    #[test]
    fn metric_range_minutes_defaults_to_one_hour() {
        assert_eq!(MetricRange::default().minutes(), 60);
    }
}
