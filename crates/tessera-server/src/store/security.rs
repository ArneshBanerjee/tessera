//! Issue security schemes, their levels, and the members of each level.

use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Scheme {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub default_level_id: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Level {
    pub id: i64,
    pub scheme_id: i64,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Member {
    pub id: i64,
    pub level_id: i64,
    pub holder_type: String,
    pub holder_parameter: Option<String>,
}

pub async fn schemes(db: &SqlitePool) -> sqlx::Result<Vec<Scheme>> {
    sqlx::query_as(
        "SELECT id, name, description, default_level_id FROM security_schemes ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(db)
    .await
}

pub async fn scheme(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Scheme>> {
    sqlx::query_as(
        "SELECT id, name, description, default_level_id FROM security_schemes WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn create_scheme(db: &SqlitePool, name: &str, description: &str) -> sqlx::Result<Scheme> {
    sqlx::query_as(
        "INSERT INTO security_schemes (name, description) VALUES (?, ?)
         RETURNING id, name, description, default_level_id",
    )
    .bind(name)
    .bind(description)
    .fetch_one(db)
    .await
}

pub async fn update_scheme(db: &SqlitePool, scheme: &Scheme) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE security_schemes SET name = ?, description = ?, default_level_id = ? WHERE id = ?",
    )
    .bind(&scheme.name)
    .bind(&scheme.description)
    .bind(scheme.default_level_id)
    .bind(scheme.id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn delete_scheme(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM security_schemes WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Ids of the projects that use a scheme.
pub async fn projects_of(db: &SqlitePool, scheme_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar("SELECT id FROM projects WHERE security_scheme_id = ? ORDER BY id")
        .bind(scheme_id)
        .fetch_all(db)
        .await
}

pub async fn project_scheme(db: &SqlitePool, project_id: i64) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar("SELECT security_scheme_id FROM projects WHERE id = ?")
        .bind(project_id)
        .fetch_one(db)
        .await
}

/// Gives a project a scheme (or none). Each issue's level moves to the level `mapping` names
/// for it, or is cleared when the mapping has none.
pub async fn set_project_scheme(
    db: &SqlitePool,
    project_id: i64,
    scheme_id: Option<i64>,
    mapping: &[(i64, Option<i64>)],
) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE projects SET security_scheme_id = ? WHERE id = ?")
        .bind(scheme_id)
        .bind(project_id)
        .execute(&mut *tx)
        .await?;
    let levels: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT security_level_id FROM issues
         WHERE project_id = ? AND security_level_id IS NOT NULL",
    )
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    for old in levels {
        let new = mapping
            .iter()
            .find(|(from, _)| *from == old)
            .and_then(|(_, to)| *to);
        sqlx::query(
            "UPDATE issues SET security_level_id = ? WHERE project_id = ? AND security_level_id = ?",
        )
        .bind(new)
        .bind(project_id)
        .bind(old)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

pub async fn levels(db: &SqlitePool, scheme_id: i64) -> sqlx::Result<Vec<Level>> {
    sqlx::query_as(
        "SELECT id, scheme_id, name, description FROM security_levels WHERE scheme_id = ? ORDER BY id",
    )
    .bind(scheme_id)
    .fetch_all(db)
    .await
}

pub async fn all_levels(db: &SqlitePool) -> sqlx::Result<Vec<Level>> {
    sqlx::query_as("SELECT id, scheme_id, name, description FROM security_levels ORDER BY id")
        .fetch_all(db)
        .await
}

pub async fn level(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Level>> {
    sqlx::query_as("SELECT id, scheme_id, name, description FROM security_levels WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn create_level(
    db: &SqlitePool,
    scheme_id: i64,
    name: &str,
    description: &str,
) -> sqlx::Result<Level> {
    sqlx::query_as(
        "INSERT INTO security_levels (scheme_id, name, description) VALUES (?, ?, ?)
         RETURNING id, scheme_id, name, description",
    )
    .bind(scheme_id)
    .bind(name)
    .bind(description)
    .fetch_one(db)
    .await
}

pub async fn update_level(db: &SqlitePool, level: &Level) -> sqlx::Result<()> {
    sqlx::query("UPDATE security_levels SET name = ?, description = ? WHERE id = ?")
        .bind(&level.name)
        .bind(&level.description)
        .bind(level.id)
        .execute(db)
        .await?;
    Ok(())
}

/// Deletes a level. Its issues move to `replace_with`, or lose their level.
pub async fn delete_level(db: &SqlitePool, id: i64, replace_with: Option<i64>) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE issues SET security_level_id = ? WHERE security_level_id = ?")
        .bind(replace_with)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE security_schemes SET default_level_id = NULL WHERE default_level_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM security_levels WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

pub async fn members(db: &SqlitePool, level_id: i64) -> sqlx::Result<Vec<Member>> {
    sqlx::query_as(
        "SELECT id, level_id, holder_type, holder_parameter FROM security_level_members
         WHERE level_id = ? ORDER BY id",
    )
    .bind(level_id)
    .fetch_all(db)
    .await
}

/// The members of every level of a scheme.
pub async fn scheme_members(db: &SqlitePool, scheme_id: i64) -> sqlx::Result<Vec<Member>> {
    sqlx::query_as(
        "SELECT m.id, m.level_id, m.holder_type, m.holder_parameter
         FROM security_level_members m JOIN security_levels l ON l.id = m.level_id
         WHERE l.scheme_id = ? ORDER BY m.id",
    )
    .bind(scheme_id)
    .fetch_all(db)
    .await
}

pub async fn add_member(
    db: &SqlitePool,
    level_id: i64,
    holder_type: &str,
    holder_parameter: Option<&str>,
) -> sqlx::Result<Member> {
    sqlx::query_as(
        "INSERT INTO security_level_members (level_id, holder_type, holder_parameter) VALUES (?, ?, ?)
         RETURNING id, level_id, holder_type, holder_parameter",
    )
    .bind(level_id)
    .bind(holder_type)
    .bind(holder_parameter)
    .fetch_one(db)
    .await
}

pub async fn delete_member(db: &SqlitePool, level_id: i64, member_id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM security_level_members WHERE id = ? AND level_id = ?")
        .bind(member_id)
        .bind(level_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_issue_level(
    db: &SqlitePool,
    issue_id: i64,
    level: Option<i64>,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE issues SET security_level_id = ? WHERE id = ?")
        .bind(level)
        .bind(issue_id)
        .execute(db)
        .await?;
    Ok(())
}
