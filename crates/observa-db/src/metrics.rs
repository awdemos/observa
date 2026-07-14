use sqlx::Row;
use uuid::Uuid;

use observa_shared::{MetricSnapshot, ObservaError, Result};

use crate::pool::Db;

fn db_err(e: impl std::fmt::Display) -> ObservaError {
    ObservaError::Database(e.to_string())
}

fn db_err_ctx(ctx: &str, e: impl std::fmt::Display) -> ObservaError {
    ObservaError::Database(format!("{ctx}: {e}"))
}

const ZSTD_MAGIC: &[u8] = b"\x28\xb5\x2f\xfd";

fn encode_payload(payload: &str, compression_enabled: bool) -> Result<Vec<u8>> {
    if !compression_enabled {
        return Ok(payload.as_bytes().to_vec());
    }
    zstd::encode_all(payload.as_bytes(), 3)
        .map_err(|e| db_err_ctx("failed to compress metric", e))
}

fn decode_payload(bytes: &[u8]) -> Result<String> {
    let is_compressed = bytes.starts_with(ZSTD_MAGIC);
    let decoded = if is_compressed {
        zstd::decode_all(bytes)
            .map_err(|e| db_err_ctx("failed to decompress metric", e))?
    } else {
        bytes.to_vec()
    };
    String::from_utf8(decoded)
        .map_err(|e| db_err_ctx("failed to decode metric payload", e))
}

pub async fn store(db: &Db, snapshot: &MetricSnapshot, compression_enabled: bool) -> Result<Uuid> {
    let id = Uuid::new_v4();
    let ts = snapshot.ts.to_rfc3339();
    let payload = serde_json::to_string(snapshot)
        .map_err(|e| db_err_ctx("failed to serialize metric", e))?;
    let payload = encode_payload(&payload, compression_enabled)?;

    sqlx::query("INSERT INTO metrics (id, ts, payload) VALUES (?, ?, ?)")
        .bind(id.to_string())
        .bind(ts)
        .bind(payload)
        .execute(db.pool())
        .await
        .map_err(db_err)?;

    Ok(id)
}

pub async fn recent(db: &Db, limit: i64) -> Result<Vec<MetricSnapshot>> {
    let rows = sqlx::query("SELECT payload FROM metrics ORDER BY ts DESC LIMIT ?")
        .bind(limit)
        .fetch_all(db.pool())
        .await
        .map_err(db_err)?;

    let mut snapshots = Vec::with_capacity(rows.len());
    for row in rows {
        let payload: Vec<u8> = row
            .try_get("payload")
            .map_err(db_err)?;
        let payload = decode_payload(&payload)?;
        let snapshot: MetricSnapshot = serde_json::from_str(&payload)
            .map_err(|e| db_err_ctx("failed to deserialize metric", e))?;
        snapshots.push(snapshot);
    }

    Ok(snapshots)
}

pub async fn recent_within(db: &Db, minutes: u64) -> Result<Vec<MetricSnapshot>> {
    let cutoff = chrono::Utc::now() - chrono::Duration::minutes(minutes as i64);
    let rows = sqlx::query("SELECT payload FROM metrics WHERE ts >= ? ORDER BY ts DESC")
        .bind(cutoff.to_rfc3339())
        .fetch_all(db.pool())
        .await
        .map_err(db_err)?;

    let mut snapshots = Vec::with_capacity(rows.len());
    for row in rows {
        let payload: Vec<u8> = row
            .try_get("payload")
            .map_err(db_err)?;
        let payload = decode_payload(&payload)?;
        let snapshot: MetricSnapshot = serde_json::from_str(&payload)
            .map_err(|e| db_err_ctx("failed to deserialize metric", e))?;
        snapshots.push(snapshot);
    }

    Ok(snapshots)
}

pub async fn prune_older_than(db: &Db, retention_days: u64) -> Result<u64> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);
    let cutoff_str = cutoff.to_rfc3339();

    let result = sqlx::query("DELETE FROM metrics WHERE ts < ?")
        .bind(cutoff_str)
        .execute(db.pool())
        .await
        .map_err(db_err)?;

    Ok(result.rows_affected())
}

pub async fn row_count(db: &Db) -> Result<i64> {
    let row = sqlx::query("SELECT COUNT(*) FROM metrics")
        .fetch_one(db.pool())
        .await
        .map_err(db_err)?;
    row.try_get::<i64, _>(0)
        .map_err(|e| ObservaError::Database(e.to_string()))
}

pub async fn vacuum(db: &Db) -> Result<()> {
    sqlx::query("VACUUM")
        .execute(db.pool())
        .await
        .map_err(db_err)?;
    Ok(())
}
