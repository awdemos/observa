use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};

use observa_shared::{ObservaError, Result};

use crate::{chat, logs, metrics};

/// A thin wrapper around a SQLx SQLite connection pool.
#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

fn build_pool_options(url: &str) -> Result<SqliteConnectOptions> {
    let rest = url
        .strip_prefix("sqlite://")
        .ok_or_else(|| ObservaError::Database(format!("unsupported database url: {url}")))?;

    let path = std::path::Path::new(rest);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ObservaError::Database(format!("failed to create db directory: {e}")))?;
    }

    Ok(SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true))
}

impl Db {
    pub async fn new(url: &str) -> Result<Self> {
        let options = build_pool_options(url)?;
        let pool = SqlitePool::connect_with(options)
            .await
            .map_err(|e| ObservaError::Database(e.to_string()))?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| ObservaError::Database(e.to_string()))?;

        Ok(Db { pool })
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

impl Db {
    pub async fn store_metric(
        &self,
        snapshot: &observa_shared::MetricSnapshot,
        compression_enabled: bool,
    ) -> Result<uuid::Uuid> {
        metrics::store(self, snapshot, compression_enabled).await
    }

    pub async fn recent_metrics(&self, limit: i64) -> Result<Vec<observa_shared::MetricSnapshot>> {
        metrics::recent(self, limit).await
    }

    pub async fn prune_metrics(&self, retention_days: u64) -> Result<u64> {
        metrics::prune_older_than(self, retention_days).await
    }

    pub async fn prune_logs(&self, retention_days: u64) -> Result<u64> {
        logs::prune_older_than(self, retention_days).await
    }

    pub async fn vacuum(&self) -> Result<()> {
        metrics::vacuum(self).await
    }

    pub async fn store_log(&self, event: &observa_shared::LogEvent) -> Result<uuid::Uuid> {
        logs::store(self, event).await
    }

    pub async fn recent_logs(&self, limit: i64) -> Result<Vec<observa_shared::LogEvent>> {
        logs::recent(self, limit).await
    }

    pub async fn create_chat_session(&self) -> Result<(uuid::Uuid, String)> {
        chat::create_session(self).await
    }

    pub async fn ensure_chat_session(
        &self,
        session_id: uuid::Uuid,
        owner_token: &str,
    ) -> Result<()> {
        chat::ensure_session(self, session_id, owner_token).await
    }

    pub async fn verify_chat_session_owner(
        &self,
        session_id: uuid::Uuid,
        owner_token: &str,
    ) -> Result<bool> {
        chat::verify_session_owner(self, session_id, owner_token).await
    }

    pub async fn store_chat_message(
        &self,
        session_id: uuid::Uuid,
        msg: &observa_shared::ChatMessage,
    ) -> Result<()> {
        chat::store_message(self, session_id, msg).await
    }

    pub async fn chat_messages(
        &self,
        session_id: uuid::Uuid,
    ) -> Result<Vec<observa_shared::ChatMessage>> {
        chat::messages_for_session(self, session_id).await
    }
}
