//! Project roles and permission schemes.

use sqlx::SqlitePool;

/// The default permission scheme's id, as in Jira.
pub const DEFAULT_SCHEME: i64 = 0;
/// The Administrators project role, which project leads get.
pub const ADMINISTRATORS: i64 = 10002;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Role {
    pub id: i64,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Scheme {
    pub id: i64,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Grant {
    pub id: i64,
    pub scheme_id: i64,
    pub permission: String,
    pub holder_type: String,
    pub holder_parameter: Option<String>,
}

// Roles.

pub async fn roles(db: &SqlitePool) -> sqlx::Result<Vec<Role>> {
    sqlx::query_as("SELECT id, name, description FROM project_roles ORDER BY id")
        .fetch_all(db)
        .await
}

pub async fn role(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Role>> {
    sqlx::query_as("SELECT id, name, description FROM project_roles WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn create_role(db: &SqlitePool, name: &str, description: &str) -> sqlx::Result<Role> {
    sqlx::query_as(
        "INSERT INTO project_roles (name, description) VALUES (?, ?) RETURNING id, name, description",
    )
    .bind(name)
    .bind(description)
    .fetch_one(db)
    .await
}

pub async fn update_role(
    db: &SqlitePool,
    id: i64,
    name: &str,
    description: &str,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE project_roles SET name = ?, description = ? WHERE id = ?")
        .bind(name)
        .bind(description)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn delete_role(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query(
        "DELETE FROM permission_grants WHERE holder_type = 'projectRole' AND holder_parameter = ?",
    )
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM project_roles WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// Users who hold a role in a project directly (not through a group).
pub async fn role_users(db: &SqlitePool, project_id: i64, role_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT user_id FROM project_role_users WHERE project_id = ? AND role_id = ? ORDER BY user_id",
    )
    .bind(project_id)
    .bind(role_id)
    .fetch_all(db)
    .await
}

/// Groups whose members hold a role in a project.
pub async fn role_groups(db: &SqlitePool, project_id: i64, role_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT group_id FROM project_role_groups WHERE project_id = ? AND role_id = ? ORDER BY group_id",
    )
    .bind(project_id)
    .bind(role_id)
    .fetch_all(db)
    .await
}

pub async fn add_role_user(
    db: &SqlitePool,
    project_id: i64,
    role_id: i64,
    user_id: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO project_role_users (project_id, role_id, user_id) VALUES (?, ?, ?)",
    )
    .bind(project_id)
    .bind(role_id)
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn add_role_group(
    db: &SqlitePool,
    project_id: i64,
    role_id: i64,
    group_id: i64,
) -> sqlx::Result<()> {
    sqlx::query("INSERT OR IGNORE INTO project_role_groups (project_id, role_id, group_id) VALUES (?, ?, ?)")
        .bind(project_id)
        .bind(role_id)
        .bind(group_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn remove_role_user(
    db: &SqlitePool,
    project_id: i64,
    role_id: i64,
    user_id: i64,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "DELETE FROM project_role_users WHERE project_id = ? AND role_id = ? AND user_id = ?",
    )
    .bind(project_id)
    .bind(role_id)
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn remove_role_group(
    db: &SqlitePool,
    project_id: i64,
    role_id: i64,
    group_id: i64,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "DELETE FROM project_role_groups WHERE project_id = ? AND role_id = ? AND group_id = ?",
    )
    .bind(project_id)
    .bind(role_id)
    .bind(group_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// The roles a user holds in a project, directly or through their groups.
pub async fn roles_of_user(
    db: &SqlitePool,
    project_id: i64,
    user_id: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT role_id FROM project_role_users WHERE project_id = ?1 AND user_id = ?2
         UNION
         SELECT r.role_id FROM project_role_groups r
         JOIN group_members m ON m.group_id = r.group_id
         WHERE r.project_id = ?1 AND m.user_id = ?2",
    )
    .bind(project_id)
    .bind(user_id)
    .fetch_all(db)
    .await
}

// Schemes.

pub async fn schemes(db: &SqlitePool) -> sqlx::Result<Vec<Scheme>> {
    sqlx::query_as("SELECT id, name, description FROM permission_schemes ORDER BY id")
        .fetch_all(db)
        .await
}

pub async fn scheme(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Scheme>> {
    sqlx::query_as("SELECT id, name, description FROM permission_schemes WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn create_scheme(db: &SqlitePool, name: &str, description: &str) -> sqlx::Result<Scheme> {
    sqlx::query_as(
        "INSERT INTO permission_schemes (name, description) VALUES (?, ?) RETURNING id, name, description",
    )
    .bind(name)
    .bind(description)
    .fetch_one(db)
    .await
}

pub async fn update_scheme(
    db: &SqlitePool,
    id: i64,
    name: &str,
    description: &str,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE permission_schemes SET name = ?, description = ? WHERE id = ?")
        .bind(name)
        .bind(description)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Deletes a scheme. Projects that used it go back to the default scheme.
pub async fn delete_scheme(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE projects SET permission_scheme_id = ? WHERE permission_scheme_id = ?")
        .bind(DEFAULT_SCHEME)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM permission_schemes WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

pub async fn grants(db: &SqlitePool, scheme_id: i64) -> sqlx::Result<Vec<Grant>> {
    sqlx::query_as(
        "SELECT id, scheme_id, permission, holder_type, holder_parameter FROM permission_grants
         WHERE scheme_id = ? ORDER BY id",
    )
    .bind(scheme_id)
    .fetch_all(db)
    .await
}

pub async fn add_grant(
    db: &SqlitePool,
    scheme_id: i64,
    permission: &str,
    holder_type: &str,
    holder_parameter: Option<&str>,
) -> sqlx::Result<Grant> {
    sqlx::query_as(
        "INSERT INTO permission_grants (scheme_id, permission, holder_type, holder_parameter)
         VALUES (?, ?, ?, ?) RETURNING id, scheme_id, permission, holder_type, holder_parameter",
    )
    .bind(scheme_id)
    .bind(permission)
    .bind(holder_type)
    .bind(holder_parameter)
    .fetch_one(db)
    .await
}

/// Returns whether the grant existed in the scheme.
pub async fn delete_grant(db: &SqlitePool, scheme_id: i64, grant_id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM permission_grants WHERE id = ? AND scheme_id = ?")
        .bind(grant_id)
        .bind(scheme_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn project_scheme(db: &SqlitePool, project_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT permission_scheme_id FROM projects WHERE id = ?")
        .bind(project_id)
        .fetch_one(db)
        .await
}

pub async fn set_project_scheme(
    db: &SqlitePool,
    project_id: i64,
    scheme_id: i64,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE projects SET permission_scheme_id = ? WHERE id = ?")
        .bind(scheme_id)
        .bind(project_id)
        .execute(db)
        .await?;
    Ok(())
}
