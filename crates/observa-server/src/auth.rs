use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::state::AppState;

/// Name of the HttpOnly cookie used to store the chat owner token.
pub const OWNER_TOKEN_COOKIE: &str = "observa_chat_owner_token";

/// Header used to present the chat owner token or dashboard API token.
pub const OWNER_TOKEN_HEADER: &str = "x-owner-token";

/// Authenticate requests to the dashboard and API.
///
/// When `state.config.dashboard_token` is configured, every request must
/// present it in one of the following ways:
///
/// * `Authorization: Bearer <token>` header
/// * `X-Owner-Token: <token>` header (also used for chat session ownership)
/// * `observa_dashboard_token` cookie
///
/// Static assets (`/assets/*`) and the health probe (`/api/health`) are
/// exempt so that the UI can load CSS/JS and container orchestrators can
/// check liveness without credentials.
///
/// When no dashboard token is configured the middleware is a no-op.  This
/// preserves test behavior while still allowing operators to enforce auth in
/// production by setting `OBSERVA_DASHBOARD_TOKEN`.
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(expected) = state.config.dashboard_token.as_ref() else {
        return next.run(request).await;
    };

    if is_exempt(&request) {
        return next.run(request).await;
    }

    let provided = bearer_token(&request)
        .or_else(|| header_token(&request, OWNER_TOKEN_HEADER))
        .or_else(|| cookie_token(&request, "observa_dashboard_token"));

    match provided {
        Some(token) if constant_time_eq(expected, &token) => next.run(request).await,
        _ => {
            let path = request.uri().path().to_string();
            tracing::warn!(path = %path, "unauthenticated request rejected");
            let mut ctx = tera::Context::new();
            ctx.insert("request_path", &path);
            ctx.insert("path", &path);
            match state.tera.render("unauthorized.html", &ctx) {
                Ok(html) => (
                    StatusCode::UNAUTHORIZED,
                    [(header::WWW_AUTHENTICATE, "Bearer")],
                    Body::from(html),
                )
                    .into_response(),
                Err(error) => {
                    tracing::error!(%error, "failed to render unauthorized page");
                    (
                        StatusCode::UNAUTHORIZED,
                        [(header::WWW_AUTHENTICATE, "Bearer")],
                        Body::from("Unauthorized"),
                    )
                        .into_response()
                }
            }
        }
    }
}

fn is_exempt(request: &Request) -> bool {
    let path = request.uri().path();
    path == "/api/health"
        || path == "/login"
        || path.starts_with("/assets/")
}

fn bearer_token(request: &Request) -> Option<String> {
    let header = request.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = header.strip_prefix("Bearer ")?;
    Some(token.trim().to_string())
}

fn header_token(request: &Request, name: &str) -> Option<String> {
    header_token_from_headers(request.headers(), name)
}

fn cookie_token(request: &Request, name: &str) -> Option<String> {
    cookie_token_from_headers(request.headers(), name)
}

/// Constant-time comparison for bearer tokens.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        result |= x ^ y;
    }
    result == 0
}

/// Extract the chat owner token from a request's headers/cookies.
///
/// Priority: `X-Owner-Token` header, then the `observa_chat_owner_token`
/// cookie. The legacy `owner_token` query parameter is intentionally not
/// read here.
pub fn chat_owner_token_from_headers(headers: &HeaderMap) -> Option<String> {
    header_token_from_headers(headers, OWNER_TOKEN_HEADER)
        .or_else(|| cookie_token_from_headers(headers, OWNER_TOKEN_COOKIE))
}

/// Extract the chat owner token from a full request.
pub fn chat_owner_token(request: &Request) -> Option<String> {
    chat_owner_token_from_headers(request.headers())
}

fn header_token_from_headers(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn cookie_token_from_headers(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    for cookie in cookie_header.split(';') {
        let mut parts = cookie.trim().splitn(2, '=');
        if parts.next()? == name {
            return parts.next().map(|s| s.to_string());
        }
    }
    None
}

/// Build a `Set-Cookie` value for the chat owner token.
pub fn owner_token_cookie(token: &str) -> String {
    // Note: Secure is intentionally omitted so the cookie works over HTTP on
    // localhost. Run Observa behind a TLS-terminating reverse proxy in production.
    format!(
        "{OWNER_TOKEN_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/"
    )
}

/// Build a `Set-Cookie` value for the dashboard auth token.
pub fn dashboard_token_cookie(token: &str) -> String {
    // Note: Secure is intentionally omitted so the login cookie works over HTTP
    // on localhost. Run Observa behind a TLS-terminating reverse proxy in
    // production.
    format!(
        "observa_dashboard_token={token}; HttpOnly; SameSite=Strict; Path=/"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::util::ServiceExt;

    #[test]
    fn bearer_token_extracts_value() {
        let request = Request::builder()
            .header(header::AUTHORIZATION, "Bearer secret-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(bearer_token(&request).as_deref(), Some("secret-token"));
    }

    #[test]
    fn bearer_token_ignores_non_bearer() {
        let request = Request::builder()
            .header(header::AUTHORIZATION, "Basic c29tZTphdXRo")
            .body(Body::empty())
            .unwrap();
        assert!(bearer_token(&request).is_none());
    }

    #[test]
    fn cookie_token_parses_cookie_header() {
        let request = Request::builder()
            .header(header::COOKIE, "a=1; observa_dashboard_token=abc123; b=2")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            cookie_token(&request, "observa_dashboard_token").as_deref(),
            Some("abc123")
        );
    }

    #[test]
    fn chat_owner_token_prefers_header_over_cookie() {
        let request = Request::builder()
            .header(header::COOKIE, "observa_chat_owner_token=cookie-token")
            .header(OWNER_TOKEN_HEADER, "header-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(chat_owner_token(&request).as_deref(), Some("header-token"));
    }

    #[test]
    fn chat_owner_token_falls_back_to_cookie() {
        let request = Request::builder()
            .header(header::COOKIE, "observa_chat_owner_token=cookie-token")
            .body(Body::empty())
            .unwrap();
        assert_eq!(chat_owner_token(&request).as_deref(), Some("cookie-token"));
    }

    #[test]
    fn constant_time_eq_matches_identical_strings() {
        assert!(constant_time_eq("same", "same"));
    }

    #[test]
    fn constant_time_eq_rejects_different_strings() {
        assert!(!constant_time_eq("same", "different"));
        assert!(!constant_time_eq("same", "Same"));
    }

    fn test_state_with_token(token: &str) -> Arc<crate::state::AppState> {
        let config = observa_config::Config {
            dashboard_token: Some(token.to_string()),
            ..Default::default()
        };
        Arc::new(crate::state::AppState::new(
            config,
            observa_bus::Bus::new(),
            None,
            None,
        ).expect("state should build"))
    }

    #[tokio::test]
    async fn auth_middleware_accepts_bearer_token_via_router() {
        let app = crate::router(test_state_with_token("secret"));
        let response = app
            .oneshot(
                axum::http::Request::get("/api/status")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_middleware_rejects_missing_token_via_router() {
        let app = crate::router(test_state_with_token("secret"));
        let response = app
            .oneshot(
                axum::http::Request::get("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_middleware_accepts_dashboard_cookie_via_router() {
        let app = crate::router(test_state_with_token("secret"));
        let response = app
            .oneshot(
                axum::http::Request::get("/api/status")
                    .header(header::COOKIE, "observa_dashboard_token=secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
