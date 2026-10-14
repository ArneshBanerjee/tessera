//! User groups.

use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Group {
    pub id: i64,
    pub group_id: String,
    pub name: String,
}

pub async fn all(db: &SqlitePool) -> sqlx::Result<Vec<Group>> {
    sqlx::query_as("SELECT id, group_id, name FROM groups ORDER BY name COLLATE NOCASE")
        .fetch_all(db)
        .await
}

/// A group by name or by Jira group id.
pub async fn find(db: &SqlitePool, name_or_id: &str) -> sqlx::Result<Option<Group>> {
    sqlx::query_as("SELECT id, group_id, name FROM groups WHERE name = ? OR group_id = ?")
        .bind(name_or_id)
        .bind(name_or_id)
        .fetch_optional(db)
        .await
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Group>> {
    sqlx::query_as("SELECT id, group_id, name FROM groups WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn create(db: &SqlitePool, name: &str) -> sqlx::Result<Group> {
    sqlx::query_as("INSERT INTO groups (group_id, name) VALUES (?, ?) RETURNING id, group_id, name")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(name)
        .fetch_one(db)
        .await
}

pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM groups WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Ids of a group's members.
pub async fn members(db: &SqlitePool, group_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar("SELECT user_id FROM group_members WHERE group_id = ? ORDER BY user_id")
        .bind(group_id)
        .fetch_all(db)
        .await
}

pub async fn add_member(db: &SqlitePool, group_id: i64, user_id: i64) -> sqlx::Result<()> {
    sqlx::query("INSERT OR IGNORE INTO group_members (group_id, user_id) VALUES (?, ?)")
        .bind(group_id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Returns whether the user was a member.
pub async fn remove_member(db: &SqlitePool, group_id: i64, user_id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM group_members WHERE group_id = ? AND user_id = ?")
        .bind(group_id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// The groups a user belongs to.
pub async fn of_user(db: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<Group>> {
    sqlx::query_as(
        "SELECT g.id, g.group_id, g.name FROM groups g
         JOIN group_members m ON m.group_id = g.id
         WHERE m.user_id = ? ORDER BY g.name COLLATE NOCASE",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
}
