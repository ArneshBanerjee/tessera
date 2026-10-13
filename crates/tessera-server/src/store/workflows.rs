//! Workflows: which statuses a project uses and how issues move between them.

use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Transition {
    pub id: i64,
    pub name: String,
    pub to_status_id: i64,
}

/// The status where new issues in this workflow start.
pub async fn initial_status(db: &SqlitePool, workflow_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT status_id FROM workflow_statuses WHERE workflow_id = ? ORDER BY is_initial DESC, status_id LIMIT 1",
    )
    .bind(workflow_id)
    .fetch_one(db)
    .await
}

/// The statuses of a workflow, in their natural order.
pub async fn statuses(db: &SqlitePool, workflow_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT status_id FROM workflow_statuses WHERE workflow_id = ? ORDER BY status_id",
    )
    .bind(workflow_id)
    .fetch_all(db)
    .await
}

/// Transitions available from `status_id`: its own outgoing transitions plus global ones.
/// Like Jira, a transition that would leave the issue in the same status is not offered.
pub async fn available(
    db: &SqlitePool,
    workflow_id: i64,
    status_id: i64,
) -> sqlx::Result<Vec<Transition>> {
    sqlx::query_as(
        "SELECT id, name, to_status_id FROM workflow_transitions
         WHERE workflow_id = ?1 AND (from_status_id IS NULL OR from_status_id = ?2) AND to_status_id != ?2
         ORDER BY id",
    )
    .bind(workflow_id)
    .bind(status_id)
    .fetch_all(db)
    .await
}
