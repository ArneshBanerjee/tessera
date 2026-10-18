//! Boards and their columns.

use sqlx::SqlitePool;

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Board {
    pub id: i64,
    pub name: String,
    /// `scrum` or `kanban`.
    pub board_type: String,
    pub filter_id: i64,
    pub project_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub status_ids: Vec<i64>,
}

macro_rules! columns {
    () => {
        "id, name, board_type, filter_id, project_id"
    };
}

/// Creates a board with one column per status category: To Do, In Progress, and Done.
/// `statuses` are `(status id, category key)` pairs of the board's workflow.
pub async fn create(
    db: &SqlitePool,
    name: &str,
    board_type: &str,
    filter_id: i64,
    project_id: Option<i64>,
    statuses: &[(i64, String)],
) -> sqlx::Result<Board> {
    let mut tx = db.begin().await?;
    let board: Board = sqlx::query_as(concat!(
        "INSERT INTO boards (name, board_type, filter_id, project_id, created_at) VALUES (?, ?, ?, ?, ?) RETURNING ",
        columns!()
    ))
    .bind(name)
    .bind(board_type)
    .bind(filter_id)
    .bind(project_id)
    .bind(now_ms())
    .fetch_one(&mut *tx)
    .await?;
    let defaults = [
        ("To Do", "new"),
        ("In Progress", "indeterminate"),
        ("Done", "done"),
    ];
    for (position, (column_name, category)) in defaults.iter().enumerate() {
        let column_id: i64 = sqlx::query_scalar(
            "INSERT INTO board_columns (board_id, position, name) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(board.id)
        .bind(position as i64)
        .bind(column_name)
        .fetch_one(&mut *tx)
        .await?;
        for (status_id, _) in statuses.iter().filter(|(_, c)| c == category) {
            sqlx::query("INSERT INTO board_column_statuses (column_id, status_id) VALUES (?, ?)")
                .bind(column_id)
                .bind(status_id)
                .execute(&mut *tx)
                .await?;
        }
    }
    super::quick_filters::add_defaults(&mut tx, board.id).await?;
    tx.commit().await?;
    Ok(board)
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Board>> {
    sqlx::query_as(concat!("SELECT ", columns!(), " FROM boards WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn all(db: &SqlitePool) -> sqlx::Result<Vec<Board>> {
    sqlx::query_as(concat!("SELECT ", columns!(), " FROM boards ORDER BY id"))
        .fetch_all(db)
        .await
}

pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM boards WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// The board's columns, left to right.
pub async fn columns(db: &SqlitePool, board_id: i64) -> sqlx::Result<Vec<Column>> {
    let rows: Vec<(i64, String, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT id, name, min_issues, max_issues FROM board_columns WHERE board_id = ? ORDER BY position",
    )
    .bind(board_id)
    .fetch_all(db)
    .await?;
    let mut result = Vec::with_capacity(rows.len());
    for (id, name, min, max) in rows {
        let status_ids = sqlx::query_scalar(
            "SELECT status_id FROM board_column_statuses WHERE column_id = ? ORDER BY status_id",
        )
        .bind(id)
        .fetch_all(db)
        .await?;
        result.push(Column {
            name,
            min,
            max,
            status_ids,
        });
    }
    Ok(result)
}

/// Every status shown in some column of the board.
pub async fn mapped_statuses(db: &SqlitePool, board_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT s.status_id FROM board_column_statuses s
         JOIN board_columns c ON c.id = s.column_id WHERE c.board_id = ?",
    )
    .bind(board_id)
    .fetch_all(db)
    .await
}

/// Adds a column for one status. With `before_last`, it goes before the last column (where
/// done work usually is); otherwise it goes at the end.
pub async fn add_status_column(
    db: &SqlitePool,
    board_id: i64,
    name: &str,
    status_id: i64,
    before_last: bool,
) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    let last: Option<i64> =
        sqlx::query_scalar("SELECT MAX(position) FROM board_columns WHERE board_id = ?")
            .bind(board_id)
            .fetch_one(&mut *tx)
            .await?;
    let position = match last {
        Some(last) if before_last => {
            sqlx::query("UPDATE board_columns SET position = position + 1 WHERE board_id = ? AND position >= ?")
                .bind(board_id)
                .bind(last)
                .execute(&mut *tx)
                .await?;
            last
        }
        Some(last) => last + 1,
        None => 0,
    };
    let column: i64 = sqlx::query_scalar(
        "INSERT INTO board_columns (board_id, position, name) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(board_id)
    .bind(position)
    .bind(name)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO board_column_statuses (column_id, status_id) VALUES (?, ?)")
        .bind(column)
        .bind(status_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}
