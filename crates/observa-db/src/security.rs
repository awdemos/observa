use chrono::{DateTime, Utc};
use observa_shared::{ObservaError, Result, SecurityAlert, Severity};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::pool::Db;

fn db_err(e: impl std::fmt::Display) -> ObservaError {
    ObservaError::Database(e.to_string())
}

fn severity_string(severity: Severity) -> &'static str {
    match severity {
        Severity::Debug => "debug",
        Severity::Info => "info",
        Severity::Warn => "warn",
        Severity::Error => "error",
        Severity::Critical => "critical",
    }
}

fn parse_severity(value: &str) -> Result<Severity> {
    match value {
        "debug" => Ok(Severity::Debug),
        "info" => Ok(Severity::Info),
        "warn" => Ok(Severity::Warn),
        "error" => Ok(Severity::Error),
        "critical" => Ok(Severity::Critical),
        other => Err(ObservaError::Database(format!(
            "unknown severity in database: {other}"
        ))),
    }
}

fn serialize_raw(raw: &Option<serde_json::Value>) -> Result<Option<String>> {
    raw.as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(db_err)
}

#[allow(clippy::too_many_arguments)]
/// Hash the alert content together with the previous hash.
fn compute_hash(
    id: Uuid,
    ts: DateTime<Utc>,
    source: &str,
    unit: Option<&str>,
    severity: Severity,
    message: &str,
    raw: &Option<serde_json::Value>,
    previous_hash: &Option<String>,
) -> Result<String> {
    let raw_str = serialize_raw(raw)?;
    let mut hasher = Sha256::new();
    hasher.update(id.to_string().as_bytes());
    hasher.update(ts.to_rfc3339().as_bytes());
    hasher.update(source.as_bytes());
    if let Some(u) = unit {
        hasher.update(u.as_bytes());
    }
    hasher.update(severity_string(severity).as_bytes());
    hasher.update(message.as_bytes());
    if let Some(r) = &raw_str {
        hasher.update(r.as_bytes());
    }
    if let Some(prev) = previous_hash {
        hasher.update(prev.as_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Return the hash of the most recently stored alert, if any.
pub async fn latest_hash(db: &Db) -> Result<Option<String>> {
    let row = sqlx::query("SELECT hash FROM security_alerts ORDER BY ts DESC, id DESC LIMIT 1")
        .fetch_optional(db.pool())
        .await
        .map_err(db_err)?;
    match row {
        Some(r) => r.try_get("hash").map_err(db_err),
        None => Ok(None),
    }
}

/// Store a security alert in the append-only table, chaining it to the
/// previous alert's hash.
#[allow(clippy::too_many_arguments)]
pub async fn store(
    db: &Db,
    id: Uuid,
    ts: DateTime<Utc>,
    source: &str,
    unit: Option<&str>,
    severity: Severity,
    message: &str,
    raw: &Option<serde_json::Value>,
) -> Result<SecurityAlert> {
    let previous_hash = latest_hash(db).await?;
    let hash = compute_hash(id, ts, source, unit, severity, message, raw, &previous_hash)?;
    let raw_str = serialize_raw(raw)?;

    sqlx::query(
        "INSERT INTO security_alerts (id, ts, source, unit, severity, message, raw, previous_hash, hash)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(ts.to_rfc3339())
    .bind(source)
    .bind(unit)
    .bind(severity_string(severity))
    .bind(message)
    .bind(raw_str)
    .bind(previous_hash.as_deref())
    .bind(&hash)
    .execute(db.pool())
    .await
    .map_err(db_err)?;

    Ok(SecurityAlert {
        id,
        ts,
        source: source.to_string(),
        unit: unit.map(|s| s.to_string()),
        severity,
        message: message.to_string(),
        raw: raw.clone(),
        previous_hash,
        hash,
    })
}

fn rows_to_alerts(rows: Vec<sqlx::sqlite::SqliteRow>) -> Result<Vec<SecurityAlert>> {
    let mut alerts = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.try_get("id").map_err(db_err)?;
        let id = id.parse::<Uuid>().map_err(db_err)?;
        let ts: DateTime<Utc> = row.try_get("ts").map_err(db_err)?;
        let source: String = row.try_get("source").map_err(db_err)?;
        let unit: Option<String> = row.try_get("unit").map_err(db_err)?;
        let severity: String = row.try_get("severity").map_err(db_err)?;
        let message: String = row.try_get("message").map_err(db_err)?;
        let raw: Option<String> = row.try_get("raw").map_err(db_err)?;
        let previous_hash: Option<String> = row.try_get("previous_hash").map_err(db_err)?;
        let hash: String = row.try_get("hash").map_err(db_err)?;

        let raw = raw
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(db_err)?;

        alerts.push(SecurityAlert {
            id,
            ts,
            source,
            unit,
            severity: parse_severity(&severity)?,
            message,
            raw,
            previous_hash,
            hash,
        });
    }
    Ok(alerts)
}

/// Read the most recent security alerts.
pub async fn recent(db: &Db, limit: i64) -> Result<Vec<SecurityAlert>> {
    let rows = sqlx::query(
        "SELECT id, ts, source, unit, severity, message, raw, previous_hash, hash
         FROM security_alerts ORDER BY ts DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(db.pool())
    .await
    .map_err(db_err)?;

    rows_to_alerts(rows)
}

/// Read security alerts filtered by severity.
pub async fn filtered(
    db: &Db,
    severities: &[Severity],
    limit: i64,
) -> Result<Vec<SecurityAlert>> {
    let mut builder: sqlx::QueryBuilder<sqlx::Sqlite> = sqlx::QueryBuilder::new(
        "SELECT id, ts, source, unit, severity, message, raw, previous_hash, hash
         FROM security_alerts WHERE 1=1",
    );

    if !severities.is_empty() {
        builder.push(" AND severity IN (");
        let mut first = true;
        for s in severities {
            if !first {
                builder.push(",");
            }
            first = false;
            builder.push_bind(severity_string(*s));
        }
        builder.push(")");
    }

    builder.push(" ORDER BY ts DESC LIMIT ");
    builder.push_bind(limit);

    let rows = builder.build().fetch_all(db.pool()).await.map_err(db_err)?;
    rows_to_alerts(rows)
}

/// Verify the integrity of the alert chain.
///
/// Returns a list of alert IDs whose stored hash does not match the recomputed
/// hash.  An empty vector means the chain is intact.
pub async fn verify_chain(db: &Db) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT id, ts, source, unit, severity, message, raw, previous_hash, hash
         FROM security_alerts ORDER BY ts ASC, id ASC",
    )
    .fetch_all(db.pool())
    .await
    .map_err(db_err)?;

    let mut broken = Vec::new();
    let mut previous_hash: Option<String> = None;

    for row in rows {
        let id: String = row.try_get("id").map_err(db_err)?;
        let id_uuid = id.parse::<Uuid>().map_err(db_err)?;
        let ts: DateTime<Utc> = row.try_get("ts").map_err(db_err)?;
        let source: String = row.try_get("source").map_err(db_err)?;
        let unit: Option<String> = row.try_get("unit").map_err(db_err)?;
        let severity: String = row.try_get("severity").map_err(db_err)?;
        let message: String = row.try_get("message").map_err(db_err)?;
        let raw: Option<String> = row.try_get("raw").map_err(db_err)?;
        let stored_prev: Option<String> = row.try_get("previous_hash").map_err(db_err)?;
        let stored_hash: String = row.try_get("hash").map_err(db_err)?;

        if stored_prev != previous_hash {
            broken.push(id.clone());
            continue;
        }

        let raw_value = raw
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(db_err)?;
        let severity_enum = parse_severity(&severity)?;
        let recomputed = compute_hash(
            id_uuid,
            ts,
            &source,
            unit.as_deref(),
            severity_enum,
            &message,
            &raw_value,
            &stored_prev,
        )?;

        if recomputed != stored_hash {
            broken.push(id);
        } else {
            previous_hash = Some(stored_hash);
        }
    }

    Ok(broken)
}
