//! Sprints, and which issues are in them.
//!
//! An issue keeps the closed sprints it was in as history, and is in at most one open
//! (future or active) sprint at a time.

use sqlx::SqlitePool;

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Sprint {
    pub id: i64,
    pub name: String,
    /// `future`, `active`, or `closed`.
    pub state: String,
    pub goal: String,
    pub start_date: Option<i64>,
    pub end_date: Option<i64>,
    pub complete_date: Option<i64>,
    pub origin_board_id: Option<i64>,
    pub sequence: i64,
}

impl Sprint {
    pub fn is_open(&self) -> bool {
        self.state != "closed"
    }
}

macro_rules! columns {
    () => {
        "id, name, state, goal, start_date, end_date, complete_date, origin_board_id, sequence"
    };
}

pub async fn create(
    db: &SqlitePool,
    name: &str,
    goal: &str,
    start_date: Option<i64>,
    end_date: Option<i64>,
    board_id: i64,
) -> sqlx::Result<Sprint> {
    sqlx::query_as(concat!(
        "INSERT INTO sprints (name, goal, start_date, end_date, origin_board_id, sequence, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM sprints WHERE origin_board_id = ?5), ?6)
         RETURNING ",
        columns!()
    ))
    .bind(name)
    .bind(goal)
    .bind(start_date)
    .bind(end_date)
    .bind(board_id)
    .bind(now_ms())
    .fetch_one(db)
    .await
}

/// Saves name, goal, dates, and state.
pub async fn save(db: &SqlitePool, sprint: &Sprint) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE sprints SET name = ?, state = ?, goal = ?, start_date = ?, end_date = ?, complete_date = ?
         WHERE id = ?",
    )
    .bind(&sprint.name)
    .bind(&sprint.state)
    .bind(&sprint.goal)
    .bind(sprint.start_date)
    .bind(sprint.end_date)
    .bind(sprint.complete_date)
    .bind(sprint.id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Sprint>> {
    sqlx::query_as(concat!("SELECT ", columns!(), " FROM sprints WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn all(db: &SqlitePool) -> sqlx::Result<Vec<Sprint>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM sprints ORDER BY sequence, id"
    ))
    .fetch_all(db)
    .await
}

/// Sprints of a board, in board order.
pub async fn of_board(db: &SqlitePool, board_id: i64) -> sqlx::Result<Vec<Sprint>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM sprints WHERE origin_board_id = ? ORDER BY sequence, id"
    ))
    .bind(board_id)
    .fetch_all(db)
    .await
}

/// Deletes a sprint. Its open issues fall back to the backlog.
pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sprints WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Swaps the board positions of two sprints.
pub async fn swap(db: &SqlitePool, a: &Sprint, b: &Sprint) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    for (id, sequence) in [(a.id, b.sequence), (b.id, a.sequence)] {
        sqlx::query("UPDATE sprints SET sequence = ? WHERE id = ?")
            .bind(sequence)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

/// All sprints an issue has been in, oldest first.
pub async fn of_issue(db: &SqlitePool, issue_id: i64) -> sqlx::Result<Vec<Sprint>> {
    sqlx::query_as(
        "SELECT s.id, s.name, s.state, s.goal, s.start_date, s.end_date, s.complete_date,
            s.origin_board_id, s.sequence
         FROM sprints s JOIN issue_sprints i ON i.sprint_id = s.id
         WHERE i.issue_id = ? ORDER BY s.sequence, s.id",
    )
    .bind(issue_id)
    .fetch_all(db)
    .await
}
