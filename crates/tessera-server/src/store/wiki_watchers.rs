//! Watchers of Confluence content and spaces.

use sqlx::SqlitePool;

/// What can be watched.
#[derive(Debug, Clone, Copy)]
pub enum Target {
    Content(i64),
    Space(i64),
}

pub async fn watch(db: &SqlitePool, target: Target, user_id: i64) -> sqlx::Result<()> {
    let (sql, id) = match target {
        Target::Content(id) => (
            "INSERT OR IGNORE INTO content_watchers (content_id, user_id) VALUES (?, ?)",
            id,
        ),
        Target::Space(id) => (
            "INSERT OR IGNORE INTO space_watchers (space_id, user_id) VALUES (?, ?)",
            id,
        ),
    };
    sqlx::query(sql).bind(id).bind(user_id).execute(db).await?;
    Ok(())
}

pub async fn unwatch(db: &SqlitePool, target: Target, user_id: i64) -> sqlx::Result<()> {
    let (sql, id) = match target {
        Target::Content(id) => (
            "DELETE FROM content_watchers WHERE content_id = ? AND user_id = ?",
            id,
        ),
        Target::Space(id) => (
            "DELETE FROM space_watchers WHERE space_id = ? AND user_id = ?",
            id,
        ),
    };
    sqlx::query(sql).bind(id).bind(user_id).execute(db).await?;
    Ok(())
}

/// Ids of the users who watch the target.
pub async fn watchers(db: &SqlitePool, target: Target) -> sqlx::Result<Vec<i64>> {
    let (sql, id) = match target {
        Target::Content(id) => (
            "SELECT user_id FROM content_watchers WHERE content_id = ? ORDER BY user_id",
            id,
        ),
        Target::Space(id) => (
            "SELECT user_id FROM space_watchers WHERE space_id = ? ORDER BY user_id",
            id,
        ),
    };
    sqlx::query_scalar(sql).bind(id).fetch_all(db).await
}

pub async fn is_watching(db: &SqlitePool, target: Target, user_id: i64) -> sqlx::Result<bool> {
    Ok(watchers(db, target).await?.contains(&user_id))
}
