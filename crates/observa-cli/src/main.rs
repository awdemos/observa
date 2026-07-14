use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use observa_bus::Bus;
use observa_cache::Cache;
use observa_collector::{spawn_collector, CollectorOpts};
use observa_config::{init_tracing, shutdown_signal, Cli, Config};
use observa_db::Db;
use observa_ingestor::{spawn_ingestor, IngestorOpts};
use observa_server::{background::spawn_background_tasks, serve_with_shutdown, AppState};
use tokio::sync::watch;
use tokio::time::timeout;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() {
    init_tracing();

    let cli = Cli::parse();
    let config = match Config::load(&cli) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("failed to load configuration: {error}");
            std::process::exit(1);
        }
    };

    info!("Observa starting on {}", config.bind_addr);
    info!(
        "persistence={} caching={}",
        if config.database_url.is_some() {
            "enabled"
        } else {
            "disabled"
        },
        if config.redis_url.is_some() {
            "enabled"
        } else {
            "disabled"
        }
    );
    if config.dashboard_token.is_none() {
        warn!(
            "OBSERVA_DASHBOARD_TOKEN is not set; the dashboard and API are open to anyone with network access"
        );
    }

    // Apply a Landlock sandbox that restricts execution to trusted system
    // directories plus the configured data directories.
    let data_dirs = data_dirs_from_config(&config);
    if let Err(err) = observa_server::tpe::enforce_tpe(&[], &data_dirs) {
        warn!(%err, "failed to enforce trusted path execution sandbox");
    }

    let db = match config.database_url.as_ref() {
        Some(url) => match Db::new(url).await {
            Ok(db) => Some(db),
            Err(error) => {
                error!(%error, "failed to open database; continuing without persistence");
                None
            }
        },
        None => None,
    };

    let cache = match Cache::new(config.redis_url.clone()).await {
        Ok(cache) => Some(cache),
        Err(error) => {
            error!(%error, "failed to open cache; continuing without caching");
            None
        }
    };

    let bus = Bus::new();
    let state = match AppState::new(config.clone(), bus.clone(), db.clone(), cache.clone()) {
        Ok(state) => Arc::new(state),
        Err(error) => {
            eprintln!("failed to build app state: {error}");
            std::process::exit(1);
        }
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let collector_handle = spawn_collector(CollectorOpts {
        interval_ms: config.sample_interval_ms,
        db: db.clone(),
        cache: cache.clone(),
        bus: bus.clone(),
        shutdown: shutdown_rx.clone(),
        compression_enabled: config.compression_enabled,
        ai_server_endpoints: config.ai_server_endpoints.clone(),
        ai_server_subnet_scan: config.ai_server_subnet_scan,
    });

    let ingestor_handle = spawn_ingestor(IngestorOpts {
        source: config.log_source.clone(),
        tail: config.log_tail,
        db,
        cache: cache.clone(),
        bus,
        shutdown: shutdown_rx.clone(),
    });

    let background_handles = spawn_background_tasks(state.clone(), shutdown_rx.clone());

    let server_handle = tokio::spawn(serve_with_shutdown(state, shutdown_signal()));

    match server_handle.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            error!(%error, "server failed");
        }
        Err(error) => {
            error!(%error, "server task panicked");
        }
    }

    info!("Observa stopping");
    let _ = shutdown_tx.send(true);

    let _ = timeout(Duration::from_secs(5), collector_handle).await;
    let _ = timeout(Duration::from_secs(5), ingestor_handle).await;

    for (idx, handle) in background_handles.into_iter().enumerate() {
        if timeout(Duration::from_secs(3), handle).await.is_err() {
            warn!(idx, "background task did not stop gracefully");
        }
    }

    info!("Observa stopped");
}

fn data_dirs_from_config(config: &observa_config::Config) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(url) = config.database_url.as_ref() {
        if let Some(path) = url.strip_prefix("sqlite://") {
            if path != ":memory:" {
                if let Some(parent) = PathBuf::from(path).parent() {
                    dirs.push(parent.to_path_buf());
                }
            }
        }
    }
    dirs
}
