//! Issue link types and issue links.

use sqlx::SqlitePool;

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkType {
    pub id: i64,
    pub name: String,
    pub inward: String,
    pub outward: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Link {
    pub id: i64,
    pub link_type_id: i64,
    pub outward_issue_id: i64,
    pub inward_issue_id: i64,
}

pub async fn types(db: &SqlitePool) -> sqlx::Result<Vec<LinkType>> {
    sqlx::query_as(
        "SELECT id, name, inward, outward FROM issue_link_types ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(db)
    .await
}

pub async fn type_by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<LinkType>> {
    sqlx::query_as("SELECT id, name, inward, outward FROM issue_link_types WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn create_type(
    db: &SqlitePool,
    name: &str,
    inward: &str,
    outward: &str,
) -> sqlx::Result<LinkType> {
    sqlx::query_as(
        "INSERT INTO issue_link_types (name, inward, outward) VALUES (?, ?, ?) RETURNING id, name, inward, outward",
    )
    .bind(name)
    .bind(inward)
    .bind(outward)
    .fetch_one(db)
    .await
}

pub async fn update_type(db: &SqlitePool, link_type: &LinkType) -> sqlx::Result<()> {
    sqlx::query("UPDATE issue_link_types SET name = ?, inward = ?, outward = ? WHERE id = ?")
        .bind(&link_type.name)
        .bind(&link_type.inward)
        .bind(&link_type.outward)
        .bind(link_type.id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn delete_type(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM issue_link_types WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Links `outward` to `inward`. Linking the same pair with the same type twice is a no-op.
pub async fn create(
    db: &SqlitePool,
    link_type_id: i64,
    outward: i64,
    inward: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO issue_links (link_type_id, outward_issue_id, inward_issue_id, created_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(link_type_id)
    .bind(outward)
    .bind(inward)
    .bind(now_ms())
    .execute(db)
    .await?;
    Ok(())
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Link>> {
    sqlx::query_as(
        "SELECT id, link_type_id, outward_issue_id, inward_issue_id FROM issue_links WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Every link that touches an issue, in either direction.
pub async fn of_issue(db: &SqlitePool, issue_id: i64) -> sqlx::Result<Vec<Link>> {
    sqlx::query_as(
        "SELECT id, link_type_id, outward_issue_id, inward_issue_id FROM issue_links
         WHERE outward_issue_id = ?1 OR inward_issue_id = ?1 ORDER BY id",
    )
    .bind(issue_id)
    .fetch_all(db)
    .await
}

pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM issue_links WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}
