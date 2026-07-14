use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{ConnectInfo, FromRequestParts, State},
    http::{request::Parts, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};

use crate::state::AppState;

/// Extractor that yields the client IP when the server is configured with
/// `into_make_service_with_connect_info`, and falls back to `127.0.0.1` for
/// unit-test requests that have no socket address. This avoids a 500 on
/// `ConnectInfo` rejection while still rate-limiting by caller IP in production.
pub struct ClientIp(pub SocketAddr);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match ConnectInfo::<SocketAddr>::from_request_parts(parts, state).await {
            Ok(ConnectInfo(addr)) => Ok(ClientIp(addr)),
            Err(_) => Ok(ClientIp(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                0,
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RateLimitConfig {
    pub max: u32,
    pub window: Duration,
}

pub const HTML_RATE_LIMIT: RateLimitConfig = RateLimitConfig {
    max: 120,
    window: Duration::from_secs(60),
};

pub const API_READ_RATE_LIMIT: RateLimitConfig = RateLimitConfig {
    max: 120,
    window: Duration::from_secs(60),
};

/// Global rate limit applied before authentication to prevent brute-force
/// attempts against the login/API token checks.
pub const GLOBAL_RATE_LIMIT: RateLimitConfig = RateLimitConfig {
    max: 300,
    window: Duration::from_secs(60),
};

#[derive(Debug, serde::Serialize)]
pub struct RateLimitError {
    pub error: String,
}

/// Check the per-IP rate limiter for *endpoint* and return `Err` when the
/// caller at *addr* has exceeded *config*.  On success the counter is
/// incremented and `Ok(())` is returned.
pub async fn rate_limit_check(
    state: &AppState,
    endpoint: &str,
    addr: SocketAddr,
    config: RateLimitConfig,
) -> Result<(), (StatusCode, Json<RateLimitError>)> {
    let now = Instant::now();
    let limiter = state.rate_limiter(endpoint);
    let mut guard = limiter.lock().await;
    let entry = guard.entry(addr).or_insert((now, 0));
    let (start, _count) = entry;
    if now.duration_since(*start) > config.window {
        *entry = (now, 0);
    }
    let (start, count) = *entry;
    if count >= config.max {
        let remaining = config.window.saturating_sub(now.duration_since(start)).as_secs();
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(RateLimitError {
                error: format!("too many requests; try again in {} seconds", remaining),
            }),
        ));
    }
    *entry = (start, count + 1);
    Ok(())
}

/// Axum middleware that enforces the global per-IP rate limit before request
/// handlers (and before the auth middleware) run.
pub async fn global_rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    ClientIp(addr): ClientIp,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    match rate_limit_check(&state, "global", addr, GLOBAL_RATE_LIMIT).await {
        Ok(()) => next.run(request).await,
        Err(resp) => resp.into_response(),
    }
}
