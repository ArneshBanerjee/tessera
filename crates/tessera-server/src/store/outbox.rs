//! Outgoing email.

use sqlx::SqlitePool;

use crate::time::now_ms;

/// Delivery stops after this many failed attempts.
pub const MAX_ATTEMPTS: i64 = 5;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Mail {
    pub id: i64,
    pub to_email: String,
    pub subject: String,
    pub body: String,
    pub attempts: i64,
}

pub async fn enqueue(
    db: &SqlitePool,
    to_email: &str,
    subject: &str,
    body: &str,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO outbox (to_email, subject, body, created_at) VALUES (?, ?, ?, ?)")
        .bind(to_email)
        .bind(subject)
        .bind(body)
        .bind(now_ms())
        .execute(db)
        .await?;
    Ok(())
}

/// Unsent mail that has attempts left, oldest first.
pub async fn pending(db: &SqlitePool, limit: i64) -> sqlx::Result<Vec<Mail>> {
    sqlx::query_as(
        "SELECT id, to_email, subject, body, attempts FROM outbox
         WHERE sent_at IS NULL AND attempts < ? ORDER BY id LIMIT ?",
    )
    .bind(MAX_ATTEMPTS)
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn mark_sent(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE outbox SET sent_at = ?, attempts = attempts + 1, last_error = NULL WHERE id = ?",
    )
    .bind(now_ms())
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn mark_failed(db: &SqlitePool, id: i64, error: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE outbox SET attempts = attempts + 1, last_error = ? WHERE id = ?")
        .bind(error)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Removes sent mail older than `before`, so the table does not grow forever.
pub async fn prune(db: &SqlitePool, before: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM outbox WHERE sent_at IS NOT NULL AND sent_at < ?")
        .bind(before)
        .execute(db)
        .await?;
    Ok(())
}
