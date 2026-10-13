//! Confluence labels on content.

use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Label {
    pub id: i64,
    pub prefix: String,
    pub name: String,
}

pub async fn for_content(db: &SqlitePool, content_id: i64) -> sqlx::Result<Vec<Label>> {
    sqlx::query_as(
        "SELECT l.id, l.prefix, l.name FROM labels l
         JOIN content_labels cl ON cl.label_id = l.id
         WHERE cl.content_id = ? ORDER BY l.name",
    )
    .bind(content_id)
    .fetch_all(db)
    .await
}

/// Adds a label to content, creating the label the first time it is used.
pub async fn add(db: &SqlitePool, content_id: i64, prefix: &str, name: &str) -> sqlx::Result<()> {
    let label_id: i64 = sqlx::query_scalar(
        "INSERT INTO labels (prefix, name) VALUES (?, ?)
         ON CONFLICT (prefix, name) DO UPDATE SET name = excluded.name RETURNING id",
    )
    .bind(prefix)
    .bind(name)
    .fetch_one(db)
    .await?;
    sqlx::query("INSERT OR IGNORE INTO content_labels (content_id, label_id) VALUES (?, ?)")
        .bind(content_id)
        .bind(label_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Removes a label from content. Returns whether the content had it.
pub async fn remove(db: &SqlitePool, content_id: i64, name: &str) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "DELETE FROM content_labels WHERE content_id = ?
         AND label_id IN (SELECT id FROM labels WHERE name = ?)",
    )
    .bind(content_id)
    .bind(name)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}
