pub mod api;
pub mod auth;
pub mod background;
pub mod chat;
pub mod insight;
pub mod llm;
pub mod llm_sanitize;
pub mod paths;
pub mod ports;
pub mod rate_limit;
pub mod routes;
pub mod state;
pub mod store;

pub use routes::router;
pub use state::{AppState, SharedState};

use std::net::SocketAddr;

use tokio::net::TcpListener;

/// Bind the HTTP server and serve the dashboard until the shutdown signal fires.
pub async fn serve(state: SharedState) -> observa_shared::Result<()> {
    serve_with_shutdown(state, observa_config::shutdown_signal()).await
}

/// Bind the HTTP server and serve the dashboard until `shutdown` resolves.
pub async fn serve_with_shutdown(
    state: SharedState,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> observa_shared::Result<()> {
    let addr: SocketAddr =
        state.config.bind_addr.parse().map_err(|e| {
            observa_shared::ObservaError::Config(format!("invalid bind address: {e}"))
        })?;

    let listener = TcpListener::bind(&addr)
        .await
        .map_err(observa_shared::ObservaError::Io)?;

    tracing::info!("dashboard listening on http://{addr}");

    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
    .map_err(observa_shared::ObservaError::Io)?;

    Ok(())
}
