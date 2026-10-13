//! Projects and project categories.

use sqlx::SqlitePool;

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Project {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub description: String,
    pub lead_id: Option<i64>,
    pub category_id: Option<i64>,
    pub url: String,
    pub assignee_type: String,
    pub project_type_key: String,
    pub workflow_id: i64,
}

/// The columns every query of this table selects, for `concat!` into static SQL.
macro_rules! columns {
    () => {
        "id, key, name, description, lead_id, category_id, url, assignee_type, project_type_key, workflow_id"
    };
}

pub struct NewProject<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub lead_id: Option<i64>,
    pub category_id: Option<i64>,
    pub url: &'a str,
    pub assignee_type: &'a str,
    pub project_type_key: &'a str,
}

/// Jira project keys: 2 to 10 characters, an uppercase letter first, then uppercase letters,
/// digits, or underscores.
pub fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    (2..=10).contains(&key.len())
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

pub async fn create(db: &SqlitePool, project: NewProject<'_>) -> sqlx::Result<Project> {
    sqlx::query_as(concat!(
        "INSERT INTO projects (key, name, description, lead_id, category_id, url, assignee_type,
            project_type_key, workflow_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?) RETURNING ",
        columns!()
    ))
    .bind(project.key)
    .bind(project.name)
    .bind(project.description)
    .bind(project.lead_id)
    .bind(project.category_id)
    .bind(project.url)
    .bind(project.assignee_type)
    .bind(project.project_type_key)
    .bind(now_ms())
    .fetch_one(db)
    .await
}

/// Finds a project by numeric id or by key, the two forms Jira accepts in URLs.
pub async fn by_id_or_key(db: &SqlitePool, id_or_key: &str) -> sqlx::Result<Option<Project>> {
    match id_or_key.parse::<i64>() {
        Ok(id) => by_id(db, id).await,
        Err(_) => {
            sqlx::query_as(concat!(
                "SELECT ",
                columns!(),
                " FROM projects WHERE key = ? COLLATE NOCASE"
            ))
            .bind(id_or_key)
            .fetch_optional(db)
            .await
        }
    }
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Project>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM projects WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn list(db: &SqlitePool) -> sqlx::Result<Vec<Project>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM projects ORDER BY name COLLATE NOCASE"
    ))
    .fetch_all(db)
    .await
}

/// Changes to a project. `None` keeps the current value.
#[derive(Default)]
pub struct ProjectUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub lead_id: Option<i64>,
    pub category_id: Option<i64>,
    pub url: Option<String>,
    pub assignee_type: Option<String>,
}

pub async fn update(db: &SqlitePool, id: i64, update: ProjectUpdate) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE projects SET
            name = COALESCE(?, name),
            description = COALESCE(?, description),
            lead_id = COALESCE(?, lead_id),
            category_id = COALESCE(?, category_id),
            url = COALESCE(?, url),
            assignee_type = COALESCE(?, assignee_type)
         WHERE id = ?",
    )
    .bind(update.name)
    .bind(update.description)
    .bind(update.lead_id)
    .bind(update.category_id)
    .bind(update.url)
    .bind(update.assignee_type)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM projects WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub description: String,
}

pub async fn categories(db: &SqlitePool) -> sqlx::Result<Vec<Category>> {
    sqlx::query_as(
        "SELECT id, name, description FROM project_categories ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(db)
    .await
}

pub async fn category(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Category>> {
    sqlx::query_as("SELECT id, name, description FROM project_categories WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn create_category(
    db: &SqlitePool,
    name: &str,
    description: &str,
) -> sqlx::Result<Category> {
    sqlx::query_as(
        "INSERT INTO project_categories (name, description) VALUES (?, ?) RETURNING id, name, description",
    )
    .bind(name)
    .bind(description)
    .fetch_one(db)
    .await
}

pub async fn update_category(
    db: &SqlitePool,
    id: i64,
    name: Option<&str>,
    description: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE project_categories SET name = COALESCE(?, name), description = COALESCE(?, description) WHERE id = ?",
    )
    .bind(name)
    .bind(description)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn delete_category(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM project_categories WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}
