//! Custom fields. The REST API names them `customfield_<id>`, and JQL also accepts `cf[<id>]`.

use serde_json::Value;
use sqlx::SqlitePool;

use crate::store::meta::json_id;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CustomField {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub field_type: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FieldOption {
    pub id: i64,
    pub field_id: i64,
    pub value: String,
    pub position: i64,
    pub disabled: bool,
}

/// The value types Tessera supports, with their Jira schema type and custom type key.
pub const FIELD_TYPES: &[(&str, &str, &str)] = &[
    (
        "string",
        "string",
        "com.atlassian.jira.plugin.system.customfieldtypes:textfield",
    ),
    (
        "text",
        "string",
        "com.atlassian.jira.plugin.system.customfieldtypes:textarea",
    ),
    (
        "number",
        "number",
        "com.atlassian.jira.plugin.system.customfieldtypes:float",
    ),
    (
        "date",
        "date",
        "com.atlassian.jira.plugin.system.customfieldtypes:datepicker",
    ),
    (
        "datetime",
        "datetime",
        "com.atlassian.jira.plugin.system.customfieldtypes:datetime",
    ),
    (
        "url",
        "string",
        "com.atlassian.jira.plugin.system.customfieldtypes:url",
    ),
    (
        "user",
        "user",
        "com.atlassian.jira.plugin.system.customfieldtypes:userpicker",
    ),
    (
        "option",
        "option",
        "com.atlassian.jira.plugin.system.customfieldtypes:select",
    ),
    (
        "options",
        "array",
        "com.atlassian.jira.plugin.system.customfieldtypes:multiselect",
    ),
    (
        "labels",
        "array",
        "com.atlassian.jira.plugin.system.customfieldtypes:labels",
    ),
];

/// Maps a Jira custom type key (from `POST /field`) to Tessera's field type.
pub fn type_from_jira_key(key: &str) -> Option<&'static str> {
    FIELD_TYPES
        .iter()
        .find(|(_, _, jira_key)| *jira_key == key)
        .map(|(name, _, _)| *name)
}

pub fn jira_schema(field_type: &str) -> (&'static str, &'static str) {
    FIELD_TYPES
        .iter()
        .find(|(name, _, _)| *name == field_type)
        .map(|(_, schema, key)| (*schema, *key))
        .unwrap_or(("string", ""))
}

pub async fn list(db: &SqlitePool) -> sqlx::Result<Vec<CustomField>> {
    sqlx::query_as("SELECT id, name, description, field_type FROM custom_fields ORDER BY id")
        .fetch_all(db)
        .await
}

pub async fn create(
    db: &SqlitePool,
    name: &str,
    description: &str,
    field_type: &str,
) -> sqlx::Result<CustomField> {
    sqlx::query_as(
        "INSERT INTO custom_fields (name, description, field_type) VALUES (?, ?, ?)
         RETURNING id, name, description, field_type",
    )
    .bind(name)
    .bind(description)
    .bind(field_type)
    .fetch_one(db)
    .await
}

pub async fn options(db: &SqlitePool, field_id: i64) -> sqlx::Result<Vec<FieldOption>> {
    sqlx::query_as(
        "SELECT id, field_id, value, position, disabled FROM custom_field_options
         WHERE field_id = ? ORDER BY position, id",
    )
    .bind(field_id)
    .fetch_all(db)
    .await
}

pub async fn all_options(db: &SqlitePool) -> sqlx::Result<Vec<FieldOption>> {
    sqlx::query_as(
        "SELECT id, field_id, value, position, disabled FROM custom_field_options ORDER BY position, id",
    )
    .fetch_all(db)
    .await
}

pub async fn add_option(db: &SqlitePool, field_id: i64, value: &str) -> sqlx::Result<FieldOption> {
    sqlx::query_as(
        "INSERT INTO custom_field_options (field_id, value, position)
         VALUES (?1, ?2, (SELECT COALESCE(MAX(position), -1) + 1 FROM custom_field_options WHERE field_id = ?1))
         RETURNING id, field_id, value, position, disabled",
    )
    .bind(field_id)
    .bind(value)
    .fetch_one(db)
    .await
}

/// Custom fields and their options, loaded once per request.
pub struct CustomCatalog {
    pub fields: Vec<CustomField>,
    pub options: Vec<FieldOption>,
}

impl CustomCatalog {
    pub async fn load(db: &SqlitePool) -> sqlx::Result<Self> {
        Ok(Self {
            fields: list(db).await?,
            options: all_options(db).await?,
        })
    }

    /// Finds a custom field by its REST key, `customfield_10010`.
    pub fn by_key(&self, key: &str) -> Option<&CustomField> {
        let id: i64 = key.strip_prefix("customfield_")?.parse().ok()?;
        self.fields.iter().find(|f| f.id == id)
    }

    pub fn option(&self, field_id: i64, id: i64) -> Option<&FieldOption> {
        self.options
            .iter()
            .find(|o| o.field_id == field_id && o.id == id)
    }

    /// Finds an enabled option by `{"id": ...}` or `{"value": ...}`.
    pub fn option_by_ref(&self, field_id: i64, reference: &Value) -> Option<&FieldOption> {
        let mut enabled = self
            .options
            .iter()
            .filter(|o| o.field_id == field_id && !o.disabled);
        match reference.get("id").and_then(json_id) {
            Some(id) => enabled.find(|o| o.id == id),
            None => {
                let value = reference.get("value")?.as_str()?;
                enabled.find(|o| o.value == value)
            }
        }
    }
}
