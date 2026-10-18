//! Confluence page restrictions.

use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Restriction {
    pub content_id: i64,
    pub operation: String,
    pub subject_type: String,
    pub subject: String,
}

pub async fn of_content(db: &SqlitePool, content_id: i64) -> sqlx::Result<Vec<Restriction>> {
    sqlx::query_as(
        "SELECT content_id, operation, subject_type, subject FROM content_restrictions
         WHERE content_id = ? ORDER BY operation, subject_type, subject",
    )
    .bind(content_id)
    .fetch_all(db)
    .await
}

pub async fn add(
    db: &SqlitePool,
    content_id: i64,
    operation: &str,
    subject_type: &str,
    subject: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO content_restrictions (content_id, operation, subject_type, subject)
         VALUES (?, ?, ?, ?)",
    )
    .bind(content_id)
    .bind(operation)
    .bind(subject_type)
    .bind(subject)
    .execute(db)
    .await?;
    Ok(())
}

/// Removes one subject from one operation. Returns whether it was there.
pub async fn remove(
    db: &SqlitePool,
    content_id: i64,
    operation: &str,
    subject_type: &str,
    subject: &str,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "DELETE FROM content_restrictions
         WHERE content_id = ? AND operation = ? AND subject_type = ? AND subject = ?",
    )
    .bind(content_id)
    .bind(operation)
    .bind(subject_type)
    .bind(subject)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes every restriction of an operation, or of all operations when `operation` is `None`.
pub async fn clear(db: &SqlitePool, content_id: i64, operation: Option<&str>) -> sqlx::Result<()> {
    sqlx::query(
        "DELETE FROM content_restrictions WHERE content_id = ? AND (? IS NULL OR operation = ?)",
    )
    .bind(content_id)
    .bind(operation)
    .bind(operation)
    .execute(db)
    .await?;
    Ok(())
}
