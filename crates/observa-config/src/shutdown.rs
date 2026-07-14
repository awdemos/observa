use tracing::{info, warn};

/// Wait for SIGINT (and SIGTERM on Unix) and return.
pub async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    tokio::pin!(ctrl_c);

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                warn!(%error, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        result = ctrl_c => {
            if let Err(error) = result {
                warn!(%error, "failed to await Ctrl+C signal");
            }
            info!("received Ctrl+C, shutting down");
        }
        () = terminate => info!("received SIGTERM, shutting down"),
    }
}
