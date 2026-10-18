//! Confluence page templates.

use sqlx::SqlitePool;

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Template {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub space_id: Option<i64>,
    /// Storage format.
    pub body: String,
    /// A JSON array of label names.
    pub labels: String,
    pub blueprint: Option<String>,
}

impl Template {
    pub fn label_names(&self) -> Vec<String> {
        serde_json::from_str(&self.labels).unwrap_or_default()
    }
}

macro_rules! columns {
    () => {
        "id, name, description, space_id, body, labels, blueprint"
    };
}

pub async fn all(db: &SqlitePool) -> sqlx::Result<Vec<Template>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM content_templates ORDER BY name COLLATE NOCASE, id"
    ))
    .fetch_all(db)
    .await
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Template>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM content_templates WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(db)
    .await
}

pub struct NewTemplate<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub space_id: Option<i64>,
    pub body: &'a str,
    pub labels: &'a [String],
}

pub async fn create(db: &SqlitePool, template: NewTemplate<'_>) -> sqlx::Result<Template> {
    sqlx::query_as(concat!(
        "INSERT INTO content_templates (name, description, space_id, body, labels, created_at)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING ",
        columns!()
    ))
    .bind(template.name)
    .bind(template.description)
    .bind(template.space_id)
    .bind(template.body)
    .bind(serde_json::to_string(template.labels).unwrap_or_else(|_| "[]".into()))
    .bind(now_ms())
    .fetch_one(db)
    .await
}

pub async fn update(db: &SqlitePool, template: &Template) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE content_templates SET name = ?, description = ?, body = ?, labels = ? WHERE id = ?",
    )
    .bind(&template.name)
    .bind(&template.description)
    .bind(&template.body)
    .bind(&template.labels)
    .bind(template.id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM content_templates WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}
