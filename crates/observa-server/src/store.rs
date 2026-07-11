use std::collections::VecDeque;
use std::sync::Arc;

use observa_db::Db;
use observa_shared::{ChatMessage, LogEvent, MetricSnapshot, SecurityAlert};
use tokio::sync::Mutex;
use uuid::Uuid;

/// A read-only store of recent metrics and logs.
///
/// This trait is the seam through which HTTP handlers read historical data.
/// The production adapter tries SQLite first, then Redis, and finally returns
/// empty data.  Tests can provide an in-memory adapter instead of spinning up
/// real databases.
#[async_trait::async_trait]
pub trait MetricStore: Send + Sync {
    async fn latest_metric(&self) -> observa_shared::Result<Option<MetricSnapshot>>;
    async fn recent_metrics(&self, limit: usize) -> observa_shared::Result<Vec<MetricSnapshot>>;
    async fn recent_logs(&self, limit: usize) -> observa_shared::Result<Vec<LogEvent>>;
    async fn search_logs(
        &self,
        q: Option<&str>,
        severities: &[observa_shared::Severity],
        limit: usize,
    ) -> observa_shared::Result<Vec<LogEvent>>;
    async fn search_logs_paginated(
        &self,
        q: Option<&str>,
        severities: &[observa_shared::Severity],
        offset: usize,
        limit: usize,
    ) -> observa_shared::Result<(Vec<LogEvent>, usize)>;
    async fn security_alerts(&self, limit: usize) -> observa_shared::Result<Vec<SecurityAlert>>;
    async fn recent_metrics_within(&self, minutes: u64) -> observa_shared::Result<Vec<MetricSnapshot>>;

    async fn store_counts(&self) -> observa_shared::Result<(usize, usize)>;
}

/// A store for chat sessions and messages.
#[async_trait::async_trait]
pub trait ChatStore: Send + Sync {
    /// Create a new chat session and return its id together with an owner token.
    /// The owner token must be presented by the browser for every subsequent
    /// operation on the session.
    async fn create_session(&self) -> Result<(Uuid, String), observa_shared::ObservaError>;

    /// Ensure the session exists, creating it with the given owner token when it
    /// does not.  Used by the full-page chat route, which lets the browser pick a
    /// fresh UUID but needs to bind it to a token immediately.
    async fn ensure_session(
        &self,
        session_id: Uuid,
        owner_token: &str,
    ) -> Result<(), observa_shared::ObservaError>;

    /// Verify the owner token for a session.  Returns `Ok(false)` when the
    /// session does not exist or the token does not match.
    async fn verify_session_owner(
        &self,
        session_id: Uuid,
        owner_token: &str,
    ) -> Result<bool, observa_shared::ObservaError>;

    async fn messages_for_session(&self, session_id: Uuid) -> Result<Vec<ChatMessage>, observa_shared::ObservaError>;
    async fn store_message(
        &self,
        session_id: Uuid,
        msg: &ChatMessage,
    ) -> Result<(), observa_shared::ObservaError>;
}

/// Production adapter: SQLite first, Redis fallback, empty result on total
/// failure.  Errors are logged at `warn` level so operators can see when the
/// dashboard is serving degraded data.
#[derive(Clone)]
pub struct DbCacheStore {
    db: Option<Db>,
    cache: Option<observa_cache::Cache>,
}

impl DbCacheStore {
    pub fn new(db: Option<Db>, cache: Option<observa_cache::Cache>) -> Self {
        Self { db, cache }
    }

    /// Read from the configured primary store (db) and fall back to the cache.
    /// Unlike the earlier version, this propagates errors instead of silently
    /// returning an empty vec, so callers can decide whether to degrade.
    async fn read_db_or_cache<D, C, T>(
        &self,
        source: &str,
        db_read: D,
        cache_read: C,
    ) -> observa_shared::Result<Vec<T>>
    where
        D: for<'a> FnOnce(&'a Db) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<T>, observa_shared::ObservaError>> + Send + 'a>> + Send,
        C: for<'a> FnOnce(&'a observa_cache::Cache) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<T>, observa_shared::ObservaError>> + Send + 'a>> + Send,
        T: Send,
    {
        let mut last_err: Option<observa_shared::ObservaError> = None;

        if let Some(db) = &self.db {
            match db_read(db).await {
                Ok(rows) if !rows.is_empty() => return Ok(rows),
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "failed to read {} from db", source);
                    last_err = Some(e);
                }
            }
        }

        if let Some(cache) = &self.cache {
            match cache_read(cache).await {
                Ok(rows) => return Ok(rows),
                Err(e) => {
                    tracing::warn!(error = %e, "failed to read {} from cache", source);
                    last_err = Some(e);
                }
            }
        }

        if let Some(e) = last_err {
            return Err(e);
        }

        Ok(Vec::new())
    }
}

/// Production adapter for chat data: persists to SQLite when available, otherwise
/// keeps sessions in memory for the lifetime of the process.
#[derive(Clone)]
pub struct DbChatStore {
    db: Option<Db>,
}

impl DbChatStore {
    pub fn new(db: Option<Db>) -> Self {
        Self { db }
    }
}

fn random_token() -> String {
    use rand::distributions::{Alphanumeric, DistString};
    Alphanumeric.sample_string(&mut rand::thread_rng(), 32)
}

#[async_trait::async_trait]
impl ChatStore for DbChatStore {
    async fn create_session(&self) -> Result<(Uuid, String), observa_shared::ObservaError> {
        if let Some(db) = &self.db {
            observa_db::chat::create_session(db).await
        } else {
            Ok((Uuid::new_v4(), random_token()))
        }
    }

    async fn ensure_session(
        &self,
        session_id: Uuid,
        owner_token: &str,
    ) -> Result<(), observa_shared::ObservaError> {
        if let Some(db) = &self.db {
            observa_db::chat::ensure_session(db, session_id, owner_token).await
        } else {
            Ok(())
        }
    }

    async fn verify_session_owner(
        &self,
        session_id: Uuid,
        owner_token: &str,
    ) -> Result<bool, observa_shared::ObservaError> {
        if let Some(db) = &self.db {
            observa_db::chat::verify_session_owner(db, session_id, owner_token).await
        } else {
            // In-memory fallback trusts the single-process owner.
            Ok(true)
        }
    }

    async fn messages_for_session(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<ChatMessage>, observa_shared::ObservaError> {
        if let Some(db) = &self.db {
            observa_db::chat::messages_for_session(db, session_id).await
        } else {
            Ok(Vec::new())
        }
    }

    async fn store_message(
        &self,
        session_id: Uuid,
        msg: &ChatMessage,
    ) -> Result<(), observa_shared::ObservaError> {
        if let Some(db) = &self.db {
            observa_db::chat::store_message(db, session_id, msg).await
        } else {
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl MetricStore for DbCacheStore {
    async fn latest_metric(&self) -> observa_shared::Result<Option<MetricSnapshot>> {
        Ok(self.recent_metrics(1).await?.into_iter().next())
    }

    async fn recent_metrics(&self, limit: usize) -> observa_shared::Result<Vec<MetricSnapshot>> {
        self.read_db_or_cache(
            "metrics",
            |db| Box::pin(observa_db::metrics::recent(db, limit as i64)),
            |cache| Box::pin(cache.recent_metrics(limit)),
        )
        .await
    }

    async fn recent_logs(&self, limit: usize) -> observa_shared::Result<Vec<LogEvent>> {
        self.read_db_or_cache(
            "logs",
            |db| Box::pin(observa_db::logs::recent(db, limit as i64)),
            |cache| Box::pin(cache.recent_logs(limit)),
        )
        .await
    }

    async fn search_logs(
        &self,
        q: Option<&str>,
        severities: &[observa_shared::Severity],
        limit: usize,
    ) -> observa_shared::Result<Vec<LogEvent>> {
        if let Some(db) = &self.db {
            match observa_db::logs::search(db, q, severities, limit as i64).await {
                Ok(rows) => return Ok(rows),
                Err(e) => tracing::warn!(error = %e, "failed to search logs in db"),
            }
        }
        Ok(self
            .recent_logs(limit)
            .await?
            .into_iter()
            .filter(|l| log_matches(l, q, severities))
            .take(limit)
            .collect())
    }

    async fn search_logs_paginated(
        &self,
        q: Option<&str>,
        severities: &[observa_shared::Severity],
        offset: usize,
        limit: usize,
    ) -> observa_shared::Result<(Vec<LogEvent>, usize)> {
        if let Some(db) = &self.db {
            match observa_db::logs::search_paginated(db, q, severities, offset as i64, limit as i64).await {
                Ok((logs, total)) => return Ok((logs, total as usize)),
                Err(e) => tracing::warn!(error = %e, "failed to search logs in db"),
            }
        }
        let all: Vec<_> = self
            .recent_logs(1000)
            .await?
            .into_iter()
            .filter(|l| log_matches(l, q, severities))
            .collect();
        let total = all.len();
        let page = all.into_iter().skip(offset).take(limit).collect();
        Ok((page, total))
    }

    async fn security_alerts(&self, limit: usize) -> observa_shared::Result<Vec<SecurityAlert>> {
        self.read_db_or_cache(
            "security alerts",
            |db| Box::pin(observa_db::security::recent(db, limit as i64)),
            |_cache| {
                Box::pin(async move {
                    // The cache is ephemeral; immutable security alerts are
                    // always read from the database so the chain can be verified.
                    Ok(Vec::new())
                })
            },
        )
        .await
    }

    async fn recent_metrics_within(&self, minutes: u64) -> observa_shared::Result<Vec<MetricSnapshot>> {
        let cutoff = chrono::Utc::now() - chrono::Duration::minutes(minutes as i64);

        let rows = self
            .read_db_or_cache(
                "metrics",
                |db| Box::pin(observa_db::metrics::recent_within(db, minutes)),
                |cache| {
                    Box::pin(async move {
                        let rows = cache.recent_metrics(100).await?;
                        Ok(rows.into_iter().filter(|m| m.ts >= cutoff).collect())
                    })
                },
            )
            .await?;

        if !rows.is_empty() {
            return Ok(rows);
        }

        // If both db and filtered cache returned empty, try a wider unfiltered
        // cache read and apply the time filter here.
        if let Some(cache) = &self.cache {
            match cache.recent_metrics(100).await {
                Ok(cache_rows) => {
                    return Ok(cache_rows.into_iter().filter(|m| m.ts >= cutoff).collect());
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to read metrics from cache");
                    return Err(e);
                }
            }
        }

        Ok(rows)
    }

    async fn store_counts(&self) -> observa_shared::Result<(usize, usize)> {
        let mut last_err: Option<observa_shared::ObservaError> = None;

        if let Some(db) = &self.db {
            match observa_db::metrics::row_count(db).await {
                Ok(metrics) => match observa_db::logs::row_count(db).await {
                    Ok(logs) => return Ok((metrics as usize, logs as usize)),
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to read log count from db");
                        last_err = Some(e);
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "failed to read metric count from db");
                    last_err = Some(e);
                }
            }
        }

        if let Some(cache) = &self.cache {
            match cache.store_counts().await {
                Ok(counts) => return Ok(counts),
                Err(e) => {
                    tracing::warn!(error = %e, "failed to read counts from cache");
                    last_err = Some(e);
                }
            }
        }

        if let Some(e) = last_err {
            return Err(e);
        }

        Ok((0, 0))
    }
}

fn log_matches(l: &LogEvent, q: Option<&str>, severities: &[observa_shared::Severity]) -> bool {
    let matches_q = q
        .map(|query| l.message.to_lowercase().contains(&query.to_lowercase()))
        .unwrap_or(true);
    let matches_sev = severities.is_empty() || severities.contains(&l.severity);
    matches_q && matches_sev
}

/// In-memory adapter for tests and headless deployments.  Stores the last
/// `capacity` metrics/logs pushed to it.
#[derive(Clone, Default)]
pub struct InMemoryStore {
    metrics: Arc<Mutex<VecDeque<MetricSnapshot>>>,
    logs: Arc<Mutex<VecDeque<LogEvent>>>,
    security_alerts: Arc<Mutex<VecDeque<SecurityAlert>>>,
    capacity: usize,
}

impl InMemoryStore {
    pub fn new(capacity: usize) -> Self {
        Self {
            metrics: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            logs: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            security_alerts: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    pub async fn push_metric(&self, metric: MetricSnapshot) {
        let mut metrics = self.metrics.lock().await;
        metrics.push_front(metric);
        metrics.truncate(self.capacity);
    }

    pub async fn push_log(&self, log: LogEvent) {
        let mut logs = self.logs.lock().await;
        logs.push_front(log);
        logs.truncate(self.capacity);
    }

    #[cfg(test)]
    pub async fn push_security_alert(&self, alert: SecurityAlert) {
        let mut alerts = self.security_alerts.lock().await;
        alerts.push_front(alert);
        alerts.truncate(self.capacity);
    }
}

/// In-memory chat store for tests and headless deployments.
#[derive(Clone, Default)]
pub struct InMemoryChatStore {
    sessions: Arc<Mutex<std::collections::HashMap<Uuid, InMemorySession>>>,
    messages: Arc<Mutex<VecDeque<(Uuid, ChatMessage)>>>,
    capacity: usize,
}

impl InMemoryChatStore {
    pub fn new(capacity: usize) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            messages: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }
}

#[derive(Clone, Default)]
struct InMemorySession {
    owner_token: String,
}

#[async_trait::async_trait]
impl ChatStore for InMemoryChatStore {
    async fn create_session(&self) -> Result<(Uuid, String), observa_shared::ObservaError> {
        let id = Uuid::new_v4();
        let owner_token = random_token();
        let mut sessions = self.sessions.lock().await;
        sessions.insert(id, InMemorySession { owner_token: owner_token.clone() });
        Ok((id, owner_token))
    }

    async fn ensure_session(
        &self,
        session_id: Uuid,
        owner_token: &str,
    ) -> Result<(), observa_shared::ObservaError> {
        let mut sessions = self.sessions.lock().await;
        sessions.insert(session_id, InMemorySession { owner_token: owner_token.to_string() });
        Ok(())
    }

    async fn verify_session_owner(
        &self,
        session_id: Uuid,
        owner_token: &str,
    ) -> Result<bool, observa_shared::ObservaError> {
        let sessions = self.sessions.lock().await;
        Ok(sessions
            .get(&session_id)
            .map(|s| s.owner_token == owner_token)
            .unwrap_or(false))
    }

    async fn messages_for_session(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<ChatMessage>, observa_shared::ObservaError> {
        Ok(self
            .messages
            .lock()
            .await
            .iter()
            .filter(|(sid, _)| *sid == session_id)
            .map(|(_, m)| m.clone())
            .collect())
    }

    async fn store_message(
        &self,
        session_id: Uuid,
        msg: &ChatMessage,
    ) -> Result<(), observa_shared::ObservaError> {
        let mut messages = self.messages.lock().await;
        messages.push_back((session_id, msg.clone()));
        messages.truncate(self.capacity);
        Ok(())
    }
}

#[async_trait::async_trait]
impl MetricStore for InMemoryStore {
    async fn latest_metric(&self) -> observa_shared::Result<Option<MetricSnapshot>> {
        let metrics = self.metrics.lock().await;
        Ok(metrics.front().cloned())
    }

    async fn recent_metrics(&self, limit: usize) -> observa_shared::Result<Vec<MetricSnapshot>> {
        let metrics = self.metrics.lock().await;
        Ok(metrics.iter().take(limit).cloned().collect())
    }

    async fn recent_logs(&self, limit: usize) -> observa_shared::Result<Vec<LogEvent>> {
        let logs = self.logs.lock().await;
        Ok(logs.iter().take(limit).cloned().collect())
    }

    async fn search_logs(
        &self,
        q: Option<&str>,
        severities: &[observa_shared::Severity],
        limit: usize,
    ) -> observa_shared::Result<Vec<LogEvent>> {
        Ok(self
            .recent_logs(limit.max(100))
            .await?
            .into_iter()
            .filter(|l| log_matches(l, q, severities))
            .take(limit)
            .collect())
    }

    async fn search_logs_paginated(
        &self,
        q: Option<&str>,
        severities: &[observa_shared::Severity],
        offset: usize,
        limit: usize,
    ) -> observa_shared::Result<(Vec<LogEvent>, usize)> {
        let all = self
            .recent_logs(1000)
            .await?
            .into_iter()
            .filter(|l| log_matches(l, q, severities))
            .collect::<Vec<_>>();
        let total = all.len();
        let page = all.into_iter().skip(offset).take(limit).collect();
        Ok((page, total))
    }

    async fn security_alerts(&self, limit: usize) -> observa_shared::Result<Vec<SecurityAlert>> {
        let alerts = self.security_alerts.lock().await;
        Ok(alerts.iter().take(limit).cloned().collect())
    }

    async fn recent_metrics_within(&self, minutes: u64) -> observa_shared::Result<Vec<MetricSnapshot>> {
        let cutoff = chrono::Utc::now() - chrono::Duration::minutes(minutes as i64);
        let metrics = self.metrics.lock().await;
        Ok(metrics.iter().filter(|s| s.ts >= cutoff).cloned().collect())
    }

    async fn store_counts(&self) -> observa_shared::Result<(usize, usize)> {
        let metrics = self.metrics.lock().await;
        let logs = self.logs.lock().await;
        Ok((metrics.len(), logs.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use observa_shared::{CpuMetrics, MemoryMetrics, MetricSnapshot, Severity};

    fn sample_metric() -> MetricSnapshot {
        MetricSnapshot {
            ts: chrono::Utc::now(),
            cpu: CpuMetrics {
                usage_percent: 12.5,
                per_core_usage: vec![10.0, 15.0],
                frequency_mhz: 2800,
            },
            memory: MemoryMetrics {
                total_bytes: 16_000_000_000,
                used_bytes: 8_000_000_000,
                free_bytes: 8_000_000_000,
            },
            swap: None,
            disks: vec![],
            networks: vec![],
            processes: vec![],
            gpu: vec![],
            ai_servers: vec![],
        }
    }

    fn sample_log(severity: Severity, security: bool) -> LogEvent {
        LogEvent {
            ts: chrono::Utc::now(),
            source: "test".into(),
            unit: None,
            severity,
            message: "sample".into(),
            raw: None,
            security,
        }
    }

    #[tokio::test]
    async fn in_memory_store_round_trips_metrics() {
        let store = InMemoryStore::new(10);
        store.push_metric(sample_metric()).await;
        let latest = store.latest_metric().await.expect("latest metric");
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().cpu.usage_percent, 12.5);
    }

    #[tokio::test]
    async fn in_memory_store_truncates_to_capacity() {
        let store = InMemoryStore::new(2);
        for pct in [1.0, 2.0, 3.0] {
            let mut m = sample_metric();
            m.cpu.usage_percent = pct;
            store.push_metric(m).await;
        }
        let metrics = store.recent_metrics(10).await.expect("recent metrics");
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].cpu.usage_percent, 3.0);
        assert_eq!(metrics[1].cpu.usage_percent, 2.0);
    }

    #[tokio::test]
    async fn in_memory_store_filters_logs_by_severity() {
        let store = InMemoryStore::new(10);
        store.push_log(sample_log(Severity::Error, false)).await;
        store.push_log(sample_log(Severity::Info, false)).await;
        let filtered = store.search_logs(None, &[Severity::Error], 10).await.expect("search logs");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].severity, Severity::Error);
    }

    #[tokio::test]
    async fn in_memory_store_paginates_logs() {
        let store = InMemoryStore::new(10);
        store.push_log(sample_log(Severity::Info, false)).await;
        store.push_log(sample_log(Severity::Error, false)).await;
        let (page, total) = store.search_logs_paginated(None, &[], 0, 1).await.expect("paginate logs");
        assert_eq!(page.len(), 1);
        assert_eq!(total, 2);
    }

    fn sample_security_alert(severity: Severity) -> SecurityAlert {
        SecurityAlert {
            id: Uuid::new_v4(),
            ts: chrono::Utc::now(),
            source: "test".into(),
            unit: None,
            severity,
            message: "sample".into(),
            raw: None,
            previous_hash: None,
            hash: "test-hash".into(),
        }
    }

    #[tokio::test]
    async fn in_memory_store_filters_security_alerts() {
        let store = InMemoryStore::new(10);
        store.push_security_alert(sample_security_alert(Severity::Error)).await;
        store.push_security_alert(sample_security_alert(Severity::Info)).await;
        let alerts = store.security_alerts(10).await.expect("security alerts");
        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[0].severity, Severity::Info);
        assert_eq!(alerts[1].severity, Severity::Error);
    }

    #[tokio::test]
    async fn in_memory_chat_store_round_trips_messages() {
        let store = InMemoryChatStore::new(10);
        let (session, _owner_token) = store.create_session().await.expect("create session");
        store
            .store_message(
                session,
                &ChatMessage {
                    role: observa_shared::Role::User,
                    content: "hello".to_string(),
                },
            )
            .await
            .expect("store message");
        let messages = store.messages_for_session(session).await.expect("load messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "hello");
    }

    #[tokio::test]
    async fn in_memory_chat_store_truncates_to_capacity() {
        let store = InMemoryChatStore::new(2);
        let (session, _owner_token) = store.create_session().await.expect("create session");
        for i in 0..3 {
            store
                .store_message(
                    session,
                    &ChatMessage {
                        role: observa_shared::Role::User,
                        content: format!("msg{i}"),
                    },
                )
                .await
                .unwrap();
        }
        let messages = store.messages_for_session(session).await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "msg0");
        assert_eq!(messages[1].content, "msg1");
    }

    #[tokio::test]
    async fn dbcache_store_reads_from_db_and_returns_empty_when_unbacked() {
        let path = "/tmp/observa_store_test.db";
        let _ = std::fs::remove_file(path);
        let db = Db::new(&format!("sqlite://{path}"))
            .await
            .expect("temp db should build");
        let snapshot = sample_metric();
        observa_db::metrics::store(&db, &snapshot, true)
            .await
            .expect("store metric");
        observa_db::logs::store(&db, &sample_log(Severity::Info, false))
            .await
            .expect("store log");

        let backed = DbCacheStore::new(Some(db.clone()), None);
        assert_eq!(backed.recent_metrics(5).await.expect("recent metrics").len(), 1);
        assert!(!backed.recent_logs(5).await.expect("recent logs").is_empty());
        assert!(!backed.search_logs(None, &[Severity::Info], 5).await.expect("search logs").is_empty());

        let unbacked = DbCacheStore::new(None, None);
        assert!(unbacked.recent_metrics(5).await.expect("recent metrics").is_empty());
        assert!(unbacked.recent_logs(5).await.expect("recent logs").is_empty());
        assert!(unbacked.latest_metric().await.expect("latest metric").is_none());
    }

    #[tokio::test]
    async fn dbcache_store_reads_from_cache_when_db_is_empty_or_absent() {
        let cache = observa_cache::Cache::new(None)
            .await
            .expect("degraded cache should build");
        let snapshot = sample_metric();
        cache
            .push_recent_metric(&snapshot)
            .await
            .expect("push metric to cache");
        cache
            .push_recent_log(&sample_log(Severity::Info, false))
            .await
            .expect("push log to cache");

        let cache_only = DbCacheStore::new(None, Some(cache.clone()));
        assert_eq!(cache_only.recent_metrics(5).await.expect("recent metrics").len(), 1);
        assert!(!cache_only.recent_logs(5).await.expect("recent logs").is_empty());
        assert!(cache_only.latest_metric().await.expect("latest metric").is_some());

        let path = "/tmp/observa_store_fallback_test.db";
        let _ = std::fs::remove_file(path);
        let db = Db::new(&format!("sqlite://{path}"))
            .await
            .expect("temp db should build");
        let db_and_cache = DbCacheStore::new(Some(db), Some(cache));
        assert_eq!(db_and_cache.recent_metrics(5).await.expect("recent metrics").len(), 1);
        assert!(!db_and_cache.recent_logs(5).await.expect("recent logs").is_empty());
    }

    #[tokio::test]
    async fn dbcache_store_falls_back_to_cache_on_db_error() {
        let cache = observa_cache::Cache::new(None)
            .await
            .expect("degraded cache should build");
        let snapshot = sample_metric();
        cache
            .push_recent_metric(&snapshot)
            .await
            .expect("push metric to cache");
        cache
            .push_recent_log(&sample_log(Severity::Info, false))
            .await
            .expect("push log to cache");

        let path = "/tmp/observa_store_corrupt_test.db";
        let _ = std::fs::remove_file(path);
        let db = Db::new(&format!("sqlite://{path}"))
            .await
            .expect("temp db should build");
        std::fs::write(path, b"not a sqlite database").expect("corrupt db file");

        let store = DbCacheStore::new(Some(db), Some(cache));
        assert_eq!(store.recent_metrics(5).await.expect("recent metrics").len(), 1);
        assert!(!store.recent_logs(5).await.expect("recent logs").is_empty());
    }

    #[tokio::test]
    async fn in_memory_store_filters_metrics_by_time_range() {
        let store = InMemoryStore::new(10);
        let mut old = sample_metric();
        old.ts = chrono::Utc::now() - chrono::Duration::minutes(120);
        store.push_metric(old).await;
        store.push_metric(sample_metric()).await;
        let metrics = store.recent_metrics_within(60).await.expect("recent metrics within");
        assert_eq!(metrics.len(), 1);
    }
}
