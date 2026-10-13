//! Saved filters: named JQL queries that people share and boards are built on.

use sqlx::SqlitePool;

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Filter {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub jql: String,
    pub owner_id: i64,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Share {
    /// `global`, `authenticated`, or `project`.
    pub share_type: String,
    pub project_id: Option<i64>,
}

macro_rules! columns {
    () => {
        "id, name, description, jql, owner_id"
    };
}

pub async fn create(
    db: &SqlitePool,
    owner_id: i64,
    name: &str,
    description: &str,
    jql: &str,
) -> sqlx::Result<Filter> {
    sqlx::query_as(concat!(
        "INSERT INTO filters (name, description, jql, owner_id, created_at) VALUES (?, ?, ?, ?, ?) RETURNING ",
        columns!()
    ))
    .bind(name)
    .bind(description)
    .bind(jql)
    .bind(owner_id)
    .bind(now_ms())
    .fetch_one(db)
    .await
}

pub async fn update(db: &SqlitePool, filter: &Filter) -> sqlx::Result<()> {
    sqlx::query("UPDATE filters SET name = ?, description = ?, jql = ? WHERE id = ?")
        .bind(&filter.name)
        .bind(&filter.description)
        .bind(&filter.jql)
        .bind(filter.id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Filter>> {
    sqlx::query_as(concat!("SELECT ", columns!(), " FROM filters WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

/// All filters. Callers decide which ones a user may see.
pub async fn all(db: &SqlitePool) -> sqlx::Result<Vec<Filter>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM filters ORDER BY name COLLATE NOCASE, id"
    ))
    .fetch_all(db)
    .await
}

pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM filters WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn shares(db: &SqlitePool, filter_id: i64) -> sqlx::Result<Vec<Share>> {
    sqlx::query_as(
        "SELECT share_type, project_id FROM filter_shares WHERE filter_id = ? ORDER BY id",
    )
    .bind(filter_id)
    .fetch_all(db)
    .await
}

pub async fn set_shares(db: &SqlitePool, filter_id: i64, shares: &[Share]) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM filter_shares WHERE filter_id = ?")
        .bind(filter_id)
        .execute(&mut *tx)
        .await?;
    for share in shares {
        sqlx::query(
            "INSERT INTO filter_shares (filter_id, share_type, project_id) VALUES (?, ?, ?)",
        )
        .bind(filter_id)
        .bind(&share.share_type)
        .bind(share.project_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

pub async fn set_favourite(
    db: &SqlitePool,
    filter_id: i64,
    user_id: i64,
    favourite: bool,
) -> sqlx::Result<()> {
    let sql = if favourite {
        "INSERT OR IGNORE INTO filter_favourites (filter_id, user_id) VALUES (?, ?)"
    } else {
        "DELETE FROM filter_favourites WHERE filter_id = ? AND user_id = ?"
    };
    sqlx::query(sql)
        .bind(filter_id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Whether `user_id` marked the filter as a favourite, and how many people did.
pub async fn favourite_state(
    db: &SqlitePool,
    filter_id: i64,
    user_id: i64,
) -> sqlx::Result<(bool, i64)> {
    sqlx::query_as(
        "SELECT COALESCE(SUM(user_id = ?), 0) > 0, COUNT(*) FROM filter_favourites WHERE filter_id = ?",
    )
    .bind(user_id)
    .bind(filter_id)
    .fetch_one(db)
    .await
}

/// Filters on a user's favourites list.
pub async fn favourites(db: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<Filter>> {
    sqlx::query_as(
        "SELECT f.id, f.name, f.description, f.jql, f.owner_id FROM filters f
         JOIN filter_favourites fav ON fav.filter_id = f.id
         WHERE fav.user_id = ? ORDER BY f.name COLLATE NOCASE",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
}

/// Whether a board uses the filter. Such filters cannot be deleted.
pub async fn used_by_board(db: &SqlitePool, filter_id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM boards WHERE filter_id = ?)")
        .bind(filter_id)
        .fetch_one(db)
        .await
}
