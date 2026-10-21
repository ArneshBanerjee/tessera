//! Bulk operations, kept so their results can be read back.

use serde_json::Value;
use sqlx::SqlitePool;

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Task {
    pub id: i64,
    pub user_id: i64,
    /// JSON.
    pub result: String,
    pub created_at: i64,
}

pub async fn record(db: &SqlitePool, user_id: i64, result: &Value) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "INSERT INTO bulk_tasks (user_id, result, created_at) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(result.to_string())
    .bind(now_ms())
    .fetch_one(db)
    .await
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Task>> {
    sqlx::query_as("SELECT id, user_id, result, created_at FROM bulk_tasks WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
}
