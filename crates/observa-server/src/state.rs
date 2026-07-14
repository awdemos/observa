use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use observa_bus::Bus;
use observa_cache::Cache;
use observa_config::Config;
use observa_db::Db;
use observa_llm::{FallbackResponder, LlmClient};
use observa_shared::{HealthStatus, InsightSnapshot};
use parking_lot::Mutex;
use tera::Tera;
use tokio::sync::RwLock;

use crate::paths::workspace_root;
use crate::store::{ChatStore, DbCacheStore, DbChatStore, MetricStore};

/// Cached explanation entry keyed by log message, with a 10-minute TTL.
#[derive(Clone)]
pub struct ExplanationEntry {
    pub created_at: DateTime<Utc>,
    pub text: String,
}

impl ExplanationEntry {
    fn new(text: String) -> Self {
        Self {
            created_at: Utc::now(),
            text,
        }
    }

    fn is_fresh(&self) -> bool {
        Utc::now().signed_duration_since(self.created_at).num_minutes() < 10
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmMode {
    /// No API key configured; use the built-in rule-based fallback.
    Fallback,
    /// API key present; route to the configured OpenAI-compatible endpoint.
    Remote,
}

/// Shared application state for the dashboard server.
pub struct AppState {
    pub config: Config,
    pub bus: Bus,
    pub db: Option<Db>,
    pub cache: Option<Cache>,
    pub store: Arc<dyn MetricStore>,
    pub chat_store: Arc<dyn ChatStore>,
    pub llm: Option<LlmClient>,
    pub fallback: Option<FallbackResponder>,
    pub llm_mode: LlmMode,
    pub tera: Tera,
    pub background: Arc<BackgroundState>,
    pub rate_limiters: Arc<Mutex<HashMap<String, RateLimiterInner>>>,
}

#[derive(Clone)]
pub struct BackgroundState {
    insight: Arc<RwLock<Option<InsightSnapshot>>>,
    health: Arc<RwLock<HealthStatus>>,
    last_alert_ts: Arc<RwLock<DateTime<Utc>>>,
    heartbeat_seq: Arc<AtomicU64>,
    log_explanations: Arc<RwLock<HashMap<String, ExplanationEntry>>>,
    acknowledged_alert_keys: Arc<RwLock<Vec<String>>>,
    /// Timestamp when CPU first exceeded the critical threshold (>=99%).
    /// Reset when CPU drops below the threshold.
    cpu_pressure_since: Arc<RwLock<Option<DateTime<Utc>>>>,
}

impl BackgroundState {
    pub fn new() -> Self {
        Self {
            insight: Arc::new(RwLock::new(None)),
            health: Arc::new(RwLock::new(HealthStatus::Healthy)),
            last_alert_ts: Arc::new(RwLock::new(DateTime::UNIX_EPOCH)),
            heartbeat_seq: Arc::new(AtomicU64::new(0)),
            log_explanations: Arc::new(RwLock::new(HashMap::new())),
            acknowledged_alert_keys: Arc::new(RwLock::new(Vec::new())),
            cpu_pressure_since: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn insight(&self) -> Option<InsightSnapshot> {
        self.insight.read().await.clone()
    }

    pub async fn set_insight(&self, value: InsightSnapshot) {
        *self.insight.write().await = Some(value);
    }

    pub async fn health(&self) -> HealthStatus {
        *self.health.read().await
    }

    pub async fn set_health(&self, value: HealthStatus) {
        *self.health.write().await = value;
    }

    pub async fn last_alert_ts(&self) -> DateTime<Utc> {
        *self.last_alert_ts.read().await
    }

    pub async fn set_last_alert_ts(&self, value: DateTime<Utc>) {
        *self.last_alert_ts.write().await = value;
    }

    pub fn next_heartbeat_seq(&self) -> u64 {
        self.heartbeat_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub async fn explanation(&self, message: &str) -> Option<String> {
        let cache = self.log_explanations.read().await;
        cache
            .get(message)
            .filter(|entry| entry.is_fresh())
            .map(|entry| entry.text.clone())
    }

    pub async fn set_explanation(&self, message: String, explanation: String) {
        self.log_explanations
            .write()
            .await
            .insert(message, ExplanationEntry::new(explanation));
    }

    pub async fn is_alert_acknowledged(&self, key: &str) -> bool {
        self.acknowledged_alert_keys.read().await.contains(&key.to_string())
    }

    pub async fn acknowledge_alert(&self, key: String) {
        let mut keys = self.acknowledged_alert_keys.write().await;
        if !keys.contains(&key) {
            keys.push(key);
        }
    }

    pub async fn list_acknowledged_alerts(&self) -> Vec<String> {
        self.acknowledged_alert_keys.read().await.clone()
    }

    pub async fn clear_acknowledged_alerts(&self) {
        self.acknowledged_alert_keys.write().await.clear();
    }

    pub async fn cpu_pressure_since(&self) -> Option<DateTime<Utc>> {
        *self.cpu_pressure_since.read().await
    }

    pub async fn set_cpu_pressure_since(&self, value: Option<DateTime<Utc>>) {
        *self.cpu_pressure_since.write().await = value;
    }
}

impl Default for BackgroundState {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(clippy::type_complexity)]
pub type RateLimiterInner = Arc<tokio::sync::Mutex<HashMap<SocketAddr, (Instant, u32)>>>;

impl AppState {
    /// Return a clone of the per-IP rate limiter arc for a named endpoint.
    /// Each endpoint gets its own isolated counter map keyed by client address.
    /// Created lazily on first access.
    pub fn rate_limiter(&self, endpoint: &str) -> RateLimiterInner {
        self.rate_limiters
            .lock()
            .entry(endpoint.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(HashMap::new())))
            .clone()
    }

    /// Clear the rate limiter state for a given endpoint (for tests).
    pub async fn clear_rate_limiter(&self, endpoint: &str) {
        let limiter = self.rate_limiters.lock().get(endpoint).cloned();
        if let Some(limiter) = limiter {
            limiter.lock().await.clear();
        }
    }

    /// Clear all rate limiter state (for test isolation).
    pub async fn clear_all_rate_limiters(&self) {
        let limiters: Vec<RateLimiterInner> = self.rate_limiters.lock().values().cloned().collect();
        for limiter in limiters {
            limiter.lock().await.clear();
        }
    }
}

impl AppState {
    pub fn new(
        config: Config,
        bus: Bus,
        db: Option<Db>,
        cache: Option<Cache>,
    ) -> observa_shared::Result<Self> {
        let has_key = config
            .llm_api_key
            .as_ref()
            .is_some_and(|k| !k.trim().is_empty());
        // Without an API key we cannot authenticate against a real OpenAI-compatible
        // endpoint, so enable the built-in rule-based fallback responder.  Setting a
        // key (even a dummy one for local llama-server) opts into the real LLM client.
        let llm_mode = if has_key { LlmMode::Remote } else { LlmMode::Fallback };
        let llm = if has_key {
            Some(LlmClient::new(
                config.llm_api_base.clone(),
                config.llm_api_key.clone(),
                config.llm_model.clone(),
                Some(std::time::Duration::from_secs(config.llm_timeout_secs)),
            ))
        } else {
            None
        };
        let fallback = if has_key { None } else { Some(FallbackResponder::new()) };
        let templates_dir = Self::templates_dir();
        let mut tera = Tera::new(&templates_dir)
            .map_err(|e| observa_shared::ObservaError::Config(format!("template error: {e}")))?;
        tera.register_filter("json_escape", json_escape_filter);
        tera.register_filter("sum", sum_filter);

        let store: Arc<dyn MetricStore> = Arc::new(DbCacheStore::new(db.clone(), cache.clone()));
        let chat_store: Arc<dyn ChatStore> = Arc::new(DbChatStore::new(db.clone()));
        let background = Arc::new(BackgroundState::new());
        let rate_limiters = Arc::new(Mutex::new(HashMap::new()));

        Ok(Self {
            config,
            bus,
            db,
            cache,
            store,
            chat_store,
            llm,
            fallback,
            llm_mode,
            tera,
            background,
            rate_limiters,
        })
    }

    fn templates_dir() -> String {
        workspace_root()
            .join("templates/**/*")
            .to_string_lossy()
            .into_owned()
    }
}

fn json_escape_filter(value: &tera::Value, _: &std::collections::HashMap<String, tera::Value>) -> tera::Result<tera::Value> {
    let s = tera::from_value::<String>(value.clone())
        .map_err(|_| tera::Error::msg("json_escape filter requires a string"))?;
    let escaped = s
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
        .replace('"', "\\\"");
    Ok(tera::to_value(escaped)?)
}

fn sum_filter(value: &tera::Value, _: &std::collections::HashMap<String, tera::Value>) -> tera::Result<tera::Value> {
    let arr = tera::from_value::<Vec<tera::Value>>(value.clone())
        .map_err(|_| tera::Error::msg("sum filter requires an array"))?;
    let total: f64 = arr.iter().filter_map(|v| v.as_f64()).sum();
    Ok(tera::to_value(total)?)
}

pub type SharedState = Arc<AppState>;

#[allow(clippy::type_complexity)]
pub type RateLimiterRegistry = Arc<Mutex<HashMap<String, RateLimiterInner>>>;
