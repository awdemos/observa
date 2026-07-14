use sqlx::Row;
use uuid::Uuid;

use observa_shared::{LogEvent, ObservaError, Result, Severity};

use crate::pool::Db;

fn db_err(e: impl std::fmt::Display) -> ObservaError {
    ObservaError::Database(e.to_string())
}

fn db_err_ctx(ctx: &str, e: impl std::fmt::Display) -> ObservaError {
    ObservaError::Database(format!("{ctx}: {e}"))
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

pub async fn store(db: &Db, log: &LogEvent) -> Result<Uuid> {
    let id = Uuid::new_v4();
    let ts = log.ts.to_rfc3339();
    let raw = log
        .raw
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| db_err_ctx("failed to serialize log raw", e))?;

    sqlx::query(
        "INSERT INTO logs (id, ts, source, unit, severity, message, raw, security) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(ts)
    .bind(&log.source)
    .bind(log.unit.as_deref())
    .bind(severity_string(log.severity))
    .bind(&log.message)
    .bind(raw)
    .bind(i32::from(log.security))
    .execute(db.pool())
    .await
    .map_err(db_err)?;

    Ok(id)
}

fn rows_to_logs(rows: Vec<sqlx::sqlite::SqliteRow>) -> Result<Vec<LogEvent>> {
    let mut logs = Vec::with_capacity(rows.len());
    for row in rows {
        let ts: chrono::DateTime<chrono::Utc> = row
            .try_get("ts")
            .map_err(db_err)?;
        let source: String = row
            .try_get("source")
            .map_err(db_err)?;
        let unit: Option<String> = row
            .try_get("unit")
            .map_err(db_err)?;
        let severity: String = row
            .try_get("severity")
            .map_err(db_err)?;
        let message: String = row
            .try_get("message")
            .map_err(db_err)?;
        let raw: Option<String> = row
            .try_get("raw")
            .map_err(db_err)?;
        let security: i32 = row
            .try_get("security")
            .map_err(db_err)?;

        let raw = raw
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(|e| db_err_ctx("failed to deserialize log raw", e))?;

        logs.push(LogEvent {
            ts,
            source,
            unit,
            severity: parse_severity(&severity)?,
            message,
            raw,
            security: security != 0,
        });
    }

    Ok(logs)
}

fn escape_like(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    for c in pattern.chars() {
        match c {
            '\\' | '%' | '_' => out.push('\\'),
            _ => {}
        }
        out.push(c);
    }
    out
}

pub async fn search(
    db: &Db,
    query: Option<&str>,
    severities: &[Severity],
    limit: i64,
) -> Result<Vec<LogEvent>> {
    let mut builder: sqlx::QueryBuilder<sqlx::Sqlite> = sqlx::QueryBuilder::new(
        "SELECT ts, source, unit, severity, message, raw, security FROM logs WHERE 1=1",
    );

    if let Some(q) = query {
        let escaped = escape_like(q);
        builder.push(" AND LOWER(message) LIKE LOWER(");
        builder.push_bind(format!("%{escaped}%"));
        builder.push(") ESCAPE '\\'");
    }

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

    let rows = builder
        .build()
        .fetch_all(db.pool())
        .await
        .map_err(db_err)?;

    rows_to_logs(rows)
}

pub async fn recent(db: &Db, limit: i64) -> Result<Vec<LogEvent>> {
    let rows = sqlx::query(
        "SELECT ts, source, unit, severity, message, raw, security FROM logs ORDER BY ts DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(db.pool())
    .await
    .map_err(db_err)?;

    rows_to_logs(rows)
}

pub async fn prune_older_than(db: &Db, retention_days: u64) -> Result<u64> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);
    let cutoff_str = cutoff.to_rfc3339();

    let result = sqlx::query("DELETE FROM logs WHERE ts < ?")
        .bind(cutoff_str)
        .execute(db.pool())
        .await
        .map_err(db_err)?;

    Ok(result.rows_affected())
}

pub async fn search_paginated(
    db: &Db,
    query: Option<&str>,
    severities: &[Severity],
    offset: i64,
    limit: i64,
) -> Result<(Vec<LogEvent>, i64)> {
    let mut count_builder: sqlx::QueryBuilder<sqlx::Sqlite> =
        sqlx::QueryBuilder::new("SELECT COUNT(*) FROM logs WHERE 1=1");
    let mut select_builder: sqlx::QueryBuilder<sqlx::Sqlite> = sqlx::QueryBuilder::new(
        "SELECT ts, source, unit, severity, message, raw, security FROM logs WHERE 1=1",
    );

    if let Some(q) = query {
        let escaped = escape_like(q);
        let like_pattern = format!("%{escaped}%");
        let clause = " AND LOWER(message) LIKE LOWER(?) ESCAPE '\\'";
        count_builder.push(clause);
        count_builder.push_bind(like_pattern.clone());
        select_builder.push(clause);
        select_builder.push_bind(like_pattern);
    }

    if !severities.is_empty() {
        let clause = " AND severity IN (";
        count_builder.push(clause);
        select_builder.push(clause);
        let mut first = true;
        for s in severities {
            if !first {
                count_builder.push(",");
                select_builder.push(",");
            }
            first = false;
            count_builder.push_bind(severity_string(*s));
            select_builder.push_bind(severity_string(*s));
        }
        count_builder.push(")");
        select_builder.push(")");
    }

    let total: i64 = count_builder
        .build()
        .fetch_one(db.pool())
        .await
        .map_err(|e| ObservaError::Database(e.to_string()))?
        .try_get(0)
        .map_err(db_err)?;

    select_builder.push(" ORDER BY ts DESC LIMIT ");
    select_builder.push_bind(limit);
    select_builder.push(" OFFSET ");
    select_builder.push_bind(offset);

    let rows = select_builder
        .build()
        .fetch_all(db.pool())
        .await
        .map_err(db_err)?;

    let logs = rows_to_logs(rows)?;
    Ok((logs, total))
}

pub async fn row_count(db: &Db) -> Result<i64> {
    let row = sqlx::query("SELECT COUNT(*) FROM logs")
        .fetch_one(db.pool())
        .await
        .map_err(db_err)?;
    row.try_get::<i64, _>(0)
        .map_err(|e| ObservaError::Database(e.to_string()))
}
