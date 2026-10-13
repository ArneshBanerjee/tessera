//! Work logged on issues.

use sqlx::SqlitePool;

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Worklog {
    pub id: i64,
    pub issue_id: i64,
    pub author_id: i64,
    pub update_author_id: i64,
    /// ADF JSON.
    pub comment: Option<String>,
    pub started: i64,
    pub time_spent_seconds: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

macro_rules! columns {
    () => {
        "id, issue_id, author_id, update_author_id, comment, started, time_spent_seconds, created_at, updated_at"
    };
}

/// Plain text of an ADF comment, for search.
fn comment_text(comment: Option<&str>) -> String {
    comment
        .and_then(|json| serde_json::from_str::<tessera_adf::Node>(json).ok())
        .map(|doc| tessera_adf::to_plain_text(&doc))
        .unwrap_or_default()
}

pub async fn create(
    db: &SqlitePool,
    issue_id: i64,
    author_id: i64,
    comment: Option<&str>,
    started: i64,
    seconds: i64,
) -> sqlx::Result<Worklog> {
    let now = now_ms();
    sqlx::query_as(concat!(
        "INSERT INTO worklogs (issue_id, author_id, update_author_id, comment, comment_text, started,
            time_spent_seconds, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING ",
        columns!()
    ))
    .bind(issue_id)
    .bind(author_id)
    .bind(author_id)
    .bind(comment)
    .bind(comment_text(comment))
    .bind(started)
    .bind(seconds)
    .bind(now)
    .bind(now)
    .fetch_one(db)
    .await
}

pub async fn update(
    db: &SqlitePool,
    id: i64,
    author_id: i64,
    comment: Option<&str>,
    started: i64,
    seconds: i64,
) -> sqlx::Result<Worklog> {
    sqlx::query_as(concat!(
        "UPDATE worklogs SET update_author_id = ?, comment = ?, comment_text = ?, started = ?,
            time_spent_seconds = ?, updated_at = ?
         WHERE id = ? RETURNING ",
        columns!()
    ))
    .bind(author_id)
    .bind(comment)
    .bind(comment_text(comment))
    .bind(started)
    .bind(seconds)
    .bind(now_ms())
    .bind(id)
    .fetch_one(db)
    .await
}

pub async fn by_id(db: &SqlitePool, issue_id: i64, id: i64) -> sqlx::Result<Option<Worklog>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM worklogs WHERE issue_id = ? AND id = ?"
    ))
    .bind(issue_id)
    .bind(id)
    .fetch_optional(db)
    .await
}

/// All worklogs of an issue, oldest first.
pub async fn of_issue(db: &SqlitePool, issue_id: i64) -> sqlx::Result<Vec<Worklog>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM worklogs WHERE issue_id = ? ORDER BY started, id"
    ))
    .bind(issue_id)
    .fetch_all(db)
    .await
}

/// Total seconds logged on an issue.
pub async fn time_spent(db: &SqlitePool, issue_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(time_spent_seconds), 0) FROM worklogs WHERE issue_id = ?",
    )
    .bind(issue_id)
    .fetch_one(db)
    .await
}

pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM worklogs WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}
