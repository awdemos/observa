use std::time::Duration;

use observa_bus::Bus;
use observa_cache::Cache;
use observa_db::Db;
use observa_shared::{Event, Result};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{debug, error, info};

use crate::ai_scanner::discover_ai_servers;
use crate::normalize::normalize;

/// Configuration handle passed to `spawn_collector`.
pub struct CollectorOpts {
    pub interval_ms: u64,
    pub db: Option<Db>,
    pub cache: Option<Cache>,
    pub bus: Bus,
    pub shutdown: watch::Receiver<bool>,
    pub compression_enabled: bool,
    pub ai_server_endpoints: Vec<String>,
    pub ai_server_subnet_scan: bool,
}

/// Spawn an asynchronous system metrics collector.
///
/// The task refreshes `sysinfo` on `interval_ms`, then stores, caches, and
/// publishes the resulting `MetricSnapshot`.  It exits cleanly when the
/// shutdown watch channel sends `true`.
pub fn spawn_collector(opts: CollectorOpts) -> JoinHandle<Result<()>> {
    tokio::spawn(async move {
        let mut system = sysinfo::System::new_all();
        let mut ticker = interval(Duration::from_millis(opts.interval_ms));

        info!(interval_ms = opts.interval_ms, "metrics collector started");

        loop {
            ticker.tick().await;

            if *opts.shutdown.borrow() {
                info!("metrics collector received shutdown signal");
                break;
            }

            system.refresh_all();
            let mut snapshot = normalize(&system);
            let discovered = discover_ai_servers(&opts.ai_server_endpoints, opts.ai_server_subnet_scan).await;
            if !discovered.is_empty() {
                snapshot.ai_servers.extend(discovered);
            }
            debug!(ts = %snapshot.ts, ai_servers = snapshot.ai_servers.len(), "collected metric snapshot");

            if let Some(db) = &opts.db {
                if let Err(error) = db.store_metric(&snapshot, opts.compression_enabled).await {
                    error!(%error, "failed to persist metric snapshot");
                }
            }

            if let Some(cache) = &opts.cache {
                if let Err(error) = cache.push_recent_metric(&snapshot).await {
                    error!(%error, "failed to cache metric snapshot");
                }
            }

            if let Err(error) = opts.bus.publish(Event::Metric(snapshot)) {
                error!(%error, "failed to publish metric event");
            }
        }

        Ok(())
    })
}

// Convenience constructor for the most common case.
impl CollectorOpts {
    /// Create a minimal collector that only publishes to the bus.
    pub fn bus_only(bus: Bus, shutdown: watch::Receiver<bool>) -> Self {
        Self {
            interval_ms: 1_000,
            db: None,
            cache: None,
            bus,
            shutdown,
            compression_enabled: true,
            ai_server_endpoints: Vec::new(),
            ai_server_subnet_scan: false,
        }
    }
}
