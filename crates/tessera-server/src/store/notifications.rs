//! In-app notifications.

use sqlx::SqlitePool;

use crate::time::now_ms;

/// How many notifications each person keeps. Older ones are deleted.
const KEEP: i64 = 500;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Notification {
    pub id: i64,
    pub user_id: i64,
    pub actor_id: Option<i64>,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub link: String,
    pub created_at: i64,
    pub read_at: Option<i64>,
}

pub struct NewNotification<'a> {
    pub user_id: i64,
    pub actor_id: Option<i64>,
    pub kind: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub link: &'a str,
}

pub async fn create(db: &SqlitePool, note: NewNotification<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO notifications (user_id, actor_id, kind, title, body, link, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(note.user_id)
    .bind(note.actor_id)
    .bind(note.kind)
    .bind(note.title)
    .bind(note.body)
    .bind(note.link)
    .bind(now_ms())
    .execute(db)
    .await?;
    sqlx::query(
        "DELETE FROM notifications WHERE user_id = ?1 AND id NOT IN
         (SELECT id FROM notifications WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2)",
    )
    .bind(note.user_id)
    .bind(KEEP)
    .execute(db)
    .await?;
    Ok(())
}

/// A person's notifications, newest first.
pub async fn of_user(
    db: &SqlitePool,
    user_id: i64,
    unread_only: bool,
    start: i64,
    limit: i64,
) -> sqlx::Result<Vec<Notification>> {
    sqlx::query_as(
        "SELECT id, user_id, actor_id, kind, title, body, link, created_at, read_at
         FROM notifications WHERE user_id = ? AND (? = 0 OR read_at IS NULL)
         ORDER BY id DESC LIMIT ? OFFSET ?",
    )
    .bind(user_id)
    .bind(unread_only)
    .bind(limit)
    .bind(start)
    .fetch_all(db)
    .await
}

pub async fn unread_count(db: &SqlitePool, user_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE user_id = ? AND read_at IS NULL")
        .bind(user_id)
        .fetch_one(db)
        .await
}

/// Marks one notification read, or unread. Returns whether it is the user's.
pub async fn set_read(db: &SqlitePool, user_id: i64, id: i64, read: bool) -> sqlx::Result<bool> {
    let result = sqlx::query("UPDATE notifications SET read_at = ? WHERE id = ? AND user_id = ?")
        .bind(read.then(now_ms))
        .bind(id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn read_all(db: &SqlitePool, user_id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE notifications SET read_at = ? WHERE user_id = ? AND read_at IS NULL")
        .bind(now_ms())
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}
