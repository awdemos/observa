use chrono::Utc;
use sqlx::Row;
use uuid::Uuid;

use observa_shared::{ChatMessage, ObservaError, Result, Role};

use crate::pool::Db;

fn role_as_str(r: Role) -> &'static str {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

fn role_from_str(s: &str) -> Result<Role> {
    match s {
        "system" => Ok(Role::System),
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        other => Err(ObservaError::Database(format!("unknown role: {other}"))),
    }
}

fn random_token() -> String {
    use rand::distributions::{Alphanumeric, DistString};
    Alphanumeric.sample_string(&mut rand::thread_rng(), 32)
}

pub async fn create_session(db: &Db) -> Result<(Uuid, String)> {
    let id = Uuid::new_v4();
    let owner_token = random_token();
    let created_at = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR IGNORE INTO chat_sessions (id, owner_token, created_at) VALUES (?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(&owner_token)
    .bind(&created_at)
    .execute(db.pool())
    .await
    .map_err(|e| ObservaError::Database(e.to_string()))?;
    Ok((id, owner_token))
}

pub async fn ensure_session(db: &Db, id: Uuid, owner_token: &str) -> Result<()> {
    let created_at = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT OR IGNORE INTO chat_sessions (id, owner_token, created_at) VALUES (?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(owner_token)
    .bind(&created_at)
    .execute(db.pool())
    .await
    .map_err(|e| ObservaError::Database(e.to_string()))?;
    Ok(())
}

pub async fn verify_session_owner(db: &Db, session_id: Uuid, owner_token: &str) -> Result<bool> {
    let row = sqlx::query(
        "SELECT owner_token FROM chat_sessions WHERE id = ?",
    )
    .bind(session_id.to_string())
    .fetch_optional(db.pool())
    .await
    .map_err(|e| ObservaError::Database(e.to_string()))?;

    match row {
        Some(row) => {
            let stored: String = row
                .try_get("owner_token")
                .map_err(|e| ObservaError::Database(e.to_string()))?;
            Ok(stored == owner_token)
        }
        None => Ok(false),
    }
}

pub async fn store_message(db: &Db, session_id: Uuid, msg: &ChatMessage) -> Result<()> {
    let id = Uuid::new_v4();
    let ts = Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO chat_messages (id, session_id, role, content, ts) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(session_id.to_string())
    .bind(role_as_str(msg.role))
    .bind(&msg.content)
    .bind(&ts)
    .execute(db.pool())
    .await
    .map_err(|e| ObservaError::Database(e.to_string()))?;

    Ok(())
}

pub async fn messages_for_session(db: &Db, session_id: Uuid) -> Result<Vec<ChatMessage>> {
    let rows =
        sqlx::query("SELECT role, content FROM chat_messages WHERE session_id = ? ORDER BY ts ASC")
            .bind(session_id.to_string())
            .fetch_all(db.pool())
            .await
            .map_err(|e| ObservaError::Database(e.to_string()))?;

    let mut messages = Vec::with_capacity(rows.len());
    for row in rows {
        let role_str: String = row
            .try_get("role")
            .map_err(|e| ObservaError::Database(e.to_string()))?;
        let content: String = row
            .try_get("content")
            .map_err(|e| ObservaError::Database(e.to_string()))?;
        messages.push(ChatMessage {
            role: role_from_str(&role_str)?,
            content,
        });
    }

    Ok(messages)
}
