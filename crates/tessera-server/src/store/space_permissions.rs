//! Confluence space permissions.

use sqlx::SqlitePool;

/// Confluence's default group, which every user joins.
pub const DEFAULT_GROUP: &str = "confluence-users";

/// Every operation a space permission can grant.
pub const OPERATIONS: &[(&str, &str)] = &[
    ("read", "space"),
    ("create", "page"),
    ("create", "blogpost"),
    ("create", "comment"),
    ("create", "attachment"),
    ("delete", "space"),
    ("delete", "page"),
    ("delete", "blogpost"),
    ("delete", "comment"),
    ("delete", "attachment"),
    ("export", "space"),
    ("restrict_content", "space"),
    ("archive", "page"),
    ("administer", "space"),
];

/// What the default group may do in a new space.
const GROUP_DEFAULTS: &[(&str, &str)] = &[
    ("read", "space"),
    ("create", "page"),
    ("create", "blogpost"),
    ("create", "comment"),
    ("create", "attachment"),
    ("delete", "space"),
    ("export", "space"),
];

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SpacePermission {
    pub id: i64,
    pub space_id: i64,
    pub subject_type: String,
    pub subject: String,
    pub operation: String,
    pub target: String,
}

pub async fn for_space(db: &SqlitePool, space_id: i64) -> sqlx::Result<Vec<SpacePermission>> {
    sqlx::query_as(
        "SELECT id, space_id, subject_type, subject, operation, target FROM space_permissions
         WHERE space_id = ? ORDER BY id",
    )
    .bind(space_id)
    .fetch_all(db)
    .await
}

/// Grants an operation. Granting it twice keeps one permission.
pub async fn add(
    db: &SqlitePool,
    space_id: i64,
    subject_type: &str,
    subject: &str,
    operation: &str,
    target: &str,
) -> sqlx::Result<SpacePermission> {
    sqlx::query_as(
        "INSERT INTO space_permissions (space_id, subject_type, subject, operation, target)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (space_id, subject_type, subject, operation, target) DO UPDATE SET target = excluded.target
         RETURNING id, space_id, subject_type, subject, operation, target",
    )
    .bind(space_id)
    .bind(subject_type)
    .bind(subject)
    .bind(operation)
    .bind(target)
    .fetch_one(db)
    .await
}

/// Returns whether the permission existed in the space.
pub async fn remove(db: &SqlitePool, space_id: i64, id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM space_permissions WHERE id = ? AND space_id = ?")
        .bind(id)
        .bind(space_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Confluence's defaults for a new space: the default group works in it, and the creator
/// may do everything.
pub async fn grant_defaults(
    db: &SqlitePool,
    space_id: i64,
    creator_account_id: &str,
) -> sqlx::Result<()> {
    for (operation, target) in GROUP_DEFAULTS {
        add(db, space_id, "group", DEFAULT_GROUP, operation, target).await?;
    }
    for (operation, target) in OPERATIONS {
        add(db, space_id, "user", creator_account_id, operation, target).await?;
    }
    Ok(())
}
