use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use observa_shared::{LogEvent, MetricSnapshot, ObservaError, Result};
use redis::aio::ConnectionManager;
use tokio::sync::Mutex;

const METRICS_KEY: &str = "observa:metrics";
const LOGS_KEY: &str = "observa:logs";

#[derive(Debug)]
struct Fallback {
    metrics: Mutex<VecDeque<String>>,
    logs: Mutex<VecDeque<String>>,
}

impl Fallback {
    fn new() -> Self {
        Self {
            metrics: Mutex::new(VecDeque::new()),
            logs: Mutex::new(VecDeque::new()),
        }
    }
}

/// A thin wrapper around Redis that degrades to an in-memory fallback when
/// Redis is unavailable or not configured.
#[derive(Clone)]
pub struct Cache {
    conn: Option<ConnectionManager>,
    fallback: Arc<Fallback>,
}

impl fmt::Debug for Cache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cache")
            .field("available", &self.conn.is_some())
            .finish_non_exhaustive()
    }
}

impl Cache {
    /// Create a cache from an optional Redis URL.  If the URL is `None` or the
    /// connection fails, the cache falls back to an in-memory store.
    pub async fn new(url: Option<String>) -> Result<Self> {
        if let Some(url) = url {
            let client = redis::Client::open(url.as_str())
                .map_err(|e| ObservaError::Cache(format!("invalid redis url: {e}")))?;

            match ConnectionManager::new(client).await {
                Ok(conn) => {
                    return Ok(Cache {
                        conn: Some(conn),
                        fallback: Arc::new(Fallback::new()),
                    })
                }
                Err(e) => {
                    tracing::warn!(error = %e, "redis unavailable; using in-memory fallback");
                }
            }
        }

        Ok(Cache {
            conn: None,
            fallback: Arc::new(Fallback::new()),
        })
    }

    pub fn is_available(&self) -> bool {
        self.conn.is_some()
    }

    pub async fn push_recent_metric(&self, metric: &MetricSnapshot) -> Result<()> {
        let payload = serde_json::to_string(metric)
            .map_err(|e| ObservaError::Cache(format!("failed to serialize metric: {e}")))?;

        if let Some(conn) = &self.conn {
            let mut conn = conn.clone();
            redis::cmd("LPUSH")
                .arg(METRICS_KEY)
                .arg(&payload)
                .query_async::<()>(&mut conn)
                .await
                .map_err(|e| ObservaError::Cache(e.to_string()))?;

            redis::cmd("LTRIM")
                .arg(METRICS_KEY)
                .arg(0)
                .arg(99)
                .query_async::<()>(&mut conn)
                .await
                .map_err(|e| ObservaError::Cache(e.to_string()))?;
        } else {
            let mut metrics = self.fallback.metrics.lock().await;
            metrics.push_front(payload);
            metrics.truncate(100);
        }

        Ok(())
    }

    pub async fn recent_metrics(&self, limit: usize) -> Result<Vec<MetricSnapshot>> {
        let payloads = if let Some(conn) = &self.conn {
            let mut conn = conn.clone();
            redis::cmd("LRANGE")
                .arg(METRICS_KEY)
                .arg(0)
                .arg(limit as i64 - 1)
                .query_async::<Vec<String>>(&mut conn)
                .await
                .map_err(|e| ObservaError::Cache(e.to_string()))?
        } else {
            let metrics = self.fallback.metrics.lock().await;
            metrics.iter().take(limit).cloned().collect()
        };

        let mut snapshots = Vec::with_capacity(payloads.len());
        for payload in payloads {
            let snapshot = serde_json::from_str(&payload)
                .map_err(|e| ObservaError::Cache(format!("failed to deserialize metric: {e}")))?;
            snapshots.push(snapshot);
        }

        Ok(snapshots)
    }

    pub async fn push_recent_log(&self, log: &LogEvent) -> Result<()> {
        let payload = serde_json::to_string(log)
            .map_err(|e| ObservaError::Cache(format!("failed to serialize log: {e}")))?;

        if let Some(conn) = &self.conn {
            let mut conn = conn.clone();
            redis::cmd("LPUSH")
                .arg(LOGS_KEY)
                .arg(&payload)
                .query_async::<()>(&mut conn)
                .await
                .map_err(|e| ObservaError::Cache(e.to_string()))?;

            redis::cmd("LTRIM")
                .arg(LOGS_KEY)
                .arg(0)
                .arg(99)
                .query_async::<()>(&mut conn)
                .await
                .map_err(|e| ObservaError::Cache(e.to_string()))?;
        } else {
            let mut logs = self.fallback.logs.lock().await;
            logs.push_front(payload);
            logs.truncate(100);
        }

        Ok(())
    }

    pub async fn recent_logs(&self, limit: usize) -> Result<Vec<LogEvent>> {
        let payloads = if let Some(conn) = &self.conn {
            let mut conn = conn.clone();
            redis::cmd("LRANGE")
                .arg(LOGS_KEY)
                .arg(0)
                .arg(limit as i64 - 1)
                .query_async::<Vec<String>>(&mut conn)
                .await
                .map_err(|e| ObservaError::Cache(e.to_string()))?
        } else {
            let logs = self.fallback.logs.lock().await;
            logs.iter().take(limit).cloned().collect()
        };

        let mut events = Vec::with_capacity(payloads.len());
        for payload in payloads {
            let event = serde_json::from_str(&payload)
                .map_err(|e| ObservaError::Cache(format!("failed to deserialize log: {e}")))?;
            events.push(event);
        }

        Ok(events)
    }

    pub async fn store_counts(&self) -> Result<(usize, usize)> {
        if let Some(conn) = &self.conn {
            let mut conn = conn.clone();
            let metrics: i64 = redis::cmd("LLEN")
                .arg(METRICS_KEY)
                .query_async(&mut conn)
                .await
                .map_err(|e| ObservaError::Cache(e.to_string()))?;
            let logs: i64 = redis::cmd("LLEN")
                .arg(LOGS_KEY)
                .query_async(&mut conn)
                .await
                .map_err(|e| ObservaError::Cache(e.to_string()))?;
            return Ok((metrics as usize, logs as usize));
        }
        let metrics = self.fallback.metrics.lock().await.len();
        let logs = self.fallback.logs.lock().await.len();
        Ok((metrics, logs))
    }
}
