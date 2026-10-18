//! Board quick filters.

use sqlx::{SqliteConnection, SqlitePool};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QuickFilter {
    pub id: i64,
    pub board_id: i64,
    pub name: String,
    pub jql: String,
    pub description: String,
    pub position: i64,
}

/// The quick filters every new board gets, as in Jira.
pub async fn add_defaults(conn: &mut SqliteConnection, board_id: i64) -> sqlx::Result<()> {
    for (position, (name, jql)) in [
        ("Only My Issues", "assignee = currentUser()"),
        ("Recently Updated", "updatedDate >= -1d"),
    ]
    .iter()
    .enumerate()
    {
        sqlx::query(
            "INSERT INTO quick_filters (board_id, name, jql, position) VALUES (?, ?, ?, ?)",
        )
        .bind(board_id)
        .bind(name)
        .bind(jql)
        .bind(position as i64)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

pub async fn of_board(db: &SqlitePool, board_id: i64) -> sqlx::Result<Vec<QuickFilter>> {
    sqlx::query_as(
        "SELECT id, board_id, name, jql, description, position FROM quick_filters
         WHERE board_id = ? ORDER BY position, id",
    )
    .bind(board_id)
    .fetch_all(db)
    .await
}

pub async fn create(
    db: &SqlitePool,
    board_id: i64,
    name: &str,
    jql: &str,
    description: &str,
) -> sqlx::Result<QuickFilter> {
    sqlx::query_as(
        "INSERT INTO quick_filters (board_id, name, jql, description, position)
         VALUES (?1, ?2, ?3, ?4, (SELECT COALESCE(MAX(position) + 1, 0) FROM quick_filters WHERE board_id = ?1))
         RETURNING id, board_id, name, jql, description, position",
    )
    .bind(board_id)
    .bind(name)
    .bind(jql)
    .bind(description)
    .fetch_one(db)
    .await
}

pub async fn update(
    db: &SqlitePool,
    id: i64,
    name: &str,
    jql: &str,
    description: &str,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE quick_filters SET name = ?, jql = ?, description = ? WHERE id = ?")
        .bind(name)
        .bind(jql)
        .bind(description)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM quick_filters WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}
