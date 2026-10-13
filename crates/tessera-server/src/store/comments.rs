//! Issue comments. Bodies are ADF JSON, with a plain-text copy for search.

use sqlx::SqlitePool;

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Comment {
    pub id: i64,
    pub issue_id: i64,
    pub author_id: i64,
    pub update_author_id: i64,
    pub body: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// The columns every query of this table selects, for `concat!` into static SQL.
macro_rules! columns {
    () => {
        "id, issue_id, author_id, update_author_id, body, created_at, updated_at"
    };
}

pub async fn create(
    db: &SqlitePool,
    issue_id: i64,
    author_id: i64,
    body: &tessera_adf::Node,
) -> sqlx::Result<Comment> {
    let now = now_ms();
    let comment = sqlx::query_as(concat!("INSERT INTO comments (issue_id, author_id, update_author_id, body, body_text, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING ", columns!()))
    .bind(issue_id)
    .bind(author_id)
    .bind(author_id)
    .bind(serde_json::to_string(body).expect("ADF serializes"))
    .bind(tessera_adf::to_plain_text(body))
    .bind(now)
    .bind(now)
    .fetch_one(db)
    .await?;
    crate::store::issues::touch(db, issue_id).await?;
    Ok(comment)
}

pub async fn update(
    db: &SqlitePool,
    id: i64,
    author_id: i64,
    body: &tessera_adf::Node,
) -> sqlx::Result<Comment> {
    sqlx::query_as(concat!(
        "UPDATE comments SET body = ?, body_text = ?, update_author_id = ?, updated_at = ?
         WHERE id = ? RETURNING ",
        columns!()
    ))
    .bind(serde_json::to_string(body).expect("ADF serializes"))
    .bind(tessera_adf::to_plain_text(body))
    .bind(author_id)
    .bind(now_ms())
    .bind(id)
    .fetch_one(db)
    .await
}

pub async fn by_id(db: &SqlitePool, issue_id: i64, id: i64) -> sqlx::Result<Option<Comment>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM comments WHERE issue_id = ? AND id = ?"
    ))
    .bind(issue_id)
    .bind(id)
    .fetch_optional(db)
    .await
}

/// A page of comments, oldest first, and the total count.
pub async fn list(
    db: &SqlitePool,
    issue_id: i64,
    start_at: i64,
    max_results: i64,
) -> sqlx::Result<(Vec<Comment>, i64)> {
    let comments = sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM comments WHERE issue_id = ? ORDER BY created_at, id LIMIT ? OFFSET ?"
    ))
    .bind(issue_id)
    .bind(max_results)
    .bind(start_at)
    .fetch_all(db)
    .await?;
    let total = sqlx::query_scalar("SELECT COUNT(*) FROM comments WHERE issue_id = ?")
        .bind(issue_id)
        .fetch_one(db)
        .await?;
    Ok((comments, total))
}

pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM comments WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}
