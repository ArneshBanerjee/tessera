//! Project components, and which issues belong to them.

use sqlx::{SqliteConnection, SqlitePool};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Component {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub description: String,
    pub lead_id: Option<i64>,
    pub assignee_type: String,
}

macro_rules! columns {
    () => {
        "id, project_id, name, description, lead_id, assignee_type"
    };
}

pub struct ComponentFields {
    pub name: String,
    pub description: String,
    pub lead_id: Option<i64>,
    pub assignee_type: String,
}

pub async fn create(
    db: &SqlitePool,
    project_id: i64,
    fields: &ComponentFields,
) -> sqlx::Result<Component> {
    sqlx::query_as(concat!(
        "INSERT INTO components (project_id, name, description, lead_id, assignee_type)
         VALUES (?, ?, ?, ?, ?) RETURNING ",
        columns!()
    ))
    .bind(project_id)
    .bind(&fields.name)
    .bind(&fields.description)
    .bind(fields.lead_id)
    .bind(&fields.assignee_type)
    .fetch_one(db)
    .await
}

pub async fn update(db: &SqlitePool, id: i64, fields: &ComponentFields) -> sqlx::Result<()> {
    sqlx::query("UPDATE components SET name = ?, description = ?, lead_id = ?, assignee_type = ? WHERE id = ?")
        .bind(&fields.name)
        .bind(&fields.description)
        .bind(fields.lead_id)
        .bind(&fields.assignee_type)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Component>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM components WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn for_project(db: &SqlitePool, project_id: i64) -> sqlx::Result<Vec<Component>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM components WHERE project_id = ? ORDER BY name COLLATE NOCASE"
    ))
    .bind(project_id)
    .fetch_all(db)
    .await
}

/// All components, for resolving JQL values.
pub async fn all(db: &SqlitePool) -> sqlx::Result<Vec<Component>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM components ORDER BY id"
    ))
    .fetch_all(db)
    .await
}

/// Deletes a component. Its issues move to `move_to`, if given.
pub async fn delete(db: &SqlitePool, id: i64, move_to: Option<i64>) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    if let Some(target) = move_to {
        sqlx::query(
            "INSERT OR IGNORE INTO issue_components (issue_id, component_id)
             SELECT issue_id, ? FROM issue_components WHERE component_id = ?",
        )
        .bind(target)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM components WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

pub async fn issue_count(db: &SqlitePool, id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM issue_components WHERE component_id = ?")
        .bind(id)
        .fetch_one(db)
        .await
}

/// The component ids of an issue.
pub async fn of_issue(db: &SqlitePool, issue_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT component_id FROM issue_components WHERE issue_id = ? ORDER BY component_id",
    )
    .bind(issue_id)
    .fetch_all(db)
    .await
}

pub async fn set_for_issue(
    conn: &mut SqliteConnection,
    issue_id: i64,
    ids: &[i64],
) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM issue_components WHERE issue_id = ?")
        .bind(issue_id)
        .execute(&mut *conn)
        .await?;
    for id in ids {
        sqlx::query(
            "INSERT OR IGNORE INTO issue_components (issue_id, component_id) VALUES (?, ?)",
        )
        .bind(issue_id)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}
