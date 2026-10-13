//! Issues, their labels, and their custom field values.

use sqlx::{SqliteConnection, SqlitePool};

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Issue {
    pub id: i64,
    pub project_id: i64,
    pub number: i64,
    pub key: String,
    pub issue_type_id: i64,
    pub summary: String,
    /// ADF JSON.
    pub description: Option<String>,
    /// ADF JSON.
    pub environment: Option<String>,
    pub status_id: i64,
    pub priority_id: Option<i64>,
    pub resolution_id: Option<i64>,
    pub resolution_date: Option<i64>,
    pub assignee_id: Option<i64>,
    pub reporter_id: Option<i64>,
    pub creator_id: i64,
    pub parent_id: Option<i64>,
    /// `yyyy-MM-dd`.
    pub due_date: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// The columns every query of this table selects, for `concat!` into static SQL.
macro_rules! columns {
    () => {
        "id, project_id, number, key, issue_type_id, summary, description, environment, \
     status_id, priority_id, resolution_id, resolution_date, assignee_id, reporter_id, creator_id, \
     parent_id, due_date, created_at, updated_at"
    };
}

/// One entry in an issue's history, as Jira shows it: field, old value, and new value.
/// `*_value` holds ids; `*_string` holds the text a person sees.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ChangeItem {
    pub field: String,
    pub field_id: String,
    pub field_type: String,
    pub from_value: Option<String>,
    pub from_string: Option<String>,
    pub to_value: Option<String>,
    pub to_string: Option<String>,
}

/// The editable state of an issue, used for both create and update.
#[derive(Debug, Clone, Default)]
pub struct IssueFields {
    pub issue_type_id: i64,
    pub summary: String,
    pub description: Option<String>,
    pub environment: Option<String>,
    pub status_id: i64,
    pub priority_id: Option<i64>,
    pub resolution_id: Option<i64>,
    pub resolution_date: Option<i64>,
    pub assignee_id: Option<i64>,
    pub reporter_id: Option<i64>,
    pub parent_id: Option<i64>,
    pub due_date: Option<String>,
    pub labels: Vec<String>,
    /// Custom field id and its JSON value. `None` clears the value.
    pub custom_values: Vec<(i64, Option<String>)>,
}

impl IssueFields {
    pub fn from_issue(issue: &Issue, labels: Vec<String>) -> Self {
        Self {
            issue_type_id: issue.issue_type_id,
            summary: issue.summary.clone(),
            description: issue.description.clone(),
            environment: issue.environment.clone(),
            status_id: issue.status_id,
            priority_id: issue.priority_id,
            resolution_id: issue.resolution_id,
            resolution_date: issue.resolution_date,
            assignee_id: issue.assignee_id,
            reporter_id: issue.reporter_id,
            parent_id: issue.parent_id,
            due_date: issue.due_date.clone(),
            labels,
            custom_values: Vec::new(),
        }
    }
}

/// Plain text of an ADF JSON document, stored next to it for text search.
fn adf_text(adf_json: Option<&str>) -> String {
    adf_json
        .and_then(|json| serde_json::from_str::<tessera_adf::Node>(json).ok())
        .map(|doc| tessera_adf::to_plain_text(&doc))
        .unwrap_or_default()
}

/// Creates an issue and gives it the next key in its project, for example `PROJ-42`.
pub async fn create(
    db: &SqlitePool,
    project_id: i64,
    creator_id: i64,
    fields: &IssueFields,
) -> sqlx::Result<Issue> {
    let mut tx = db.begin().await?;
    let (number, project_key): (i64, String) = sqlx::query_as(
        "UPDATE projects SET issue_counter = issue_counter + 1 WHERE id = ? RETURNING issue_counter, key",
    )
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    let now = now_ms();
    let issue: Issue = sqlx::query_as(concat!("INSERT INTO issues (project_id, number, key, issue_type_id, summary, description, environment,
            description_text, environment_text, status_id, priority_id, resolution_id, resolution_date,
            assignee_id, reporter_id, creator_id, parent_id, due_date, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         RETURNING ", columns!()))
    .bind(project_id)
    .bind(number)
    .bind(format!("{project_key}-{number}"))
    .bind(fields.issue_type_id)
    .bind(&fields.summary)
    .bind(&fields.description)
    .bind(&fields.environment)
    .bind(adf_text(fields.description.as_deref()))
    .bind(adf_text(fields.environment.as_deref()))
    .bind(fields.status_id)
    .bind(fields.priority_id)
    .bind(fields.resolution_id)
    .bind(fields.resolution_date)
    .bind(fields.assignee_id)
    .bind(fields.reporter_id)
    .bind(creator_id)
    .bind(fields.parent_id)
    .bind(&fields.due_date)
    .bind(now)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    replace_labels(&mut tx, issue.id, &fields.labels).await?;
    write_custom_values(&mut tx, issue.id, &fields.custom_values).await?;
    tx.commit().await?;
    Ok(issue)
}

/// Saves new field values and records `changes` in the issue history, in one transaction.
pub async fn update(
    db: &SqlitePool,
    issue_id: i64,
    author_id: i64,
    fields: &IssueFields,
    changes: &[ChangeItem],
) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    let now = now_ms();
    sqlx::query(
        "UPDATE issues SET issue_type_id = ?, summary = ?, description = ?, environment = ?,
            description_text = ?, environment_text = ?, status_id = ?, priority_id = ?,
            resolution_id = ?, resolution_date = ?, assignee_id = ?, reporter_id = ?, parent_id = ?,
            due_date = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(fields.issue_type_id)
    .bind(&fields.summary)
    .bind(&fields.description)
    .bind(&fields.environment)
    .bind(adf_text(fields.description.as_deref()))
    .bind(adf_text(fields.environment.as_deref()))
    .bind(fields.status_id)
    .bind(fields.priority_id)
    .bind(fields.resolution_id)
    .bind(fields.resolution_date)
    .bind(fields.assignee_id)
    .bind(fields.reporter_id)
    .bind(fields.parent_id)
    .bind(&fields.due_date)
    .bind(now)
    .bind(issue_id)
    .execute(&mut *tx)
    .await?;
    replace_labels(&mut tx, issue_id, &fields.labels).await?;
    write_custom_values(&mut tx, issue_id, &fields.custom_values).await?;
    if !changes.is_empty() {
        crate::store::changelog::record(&mut tx, issue_id, author_id, now, changes).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn replace_labels(
    tx: &mut SqliteConnection,
    issue_id: i64,
    labels: &[String],
) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM issue_labels WHERE issue_id = ?")
        .bind(issue_id)
        .execute(&mut *tx)
        .await?;
    for label in labels {
        sqlx::query("INSERT OR IGNORE INTO issue_labels (issue_id, label) VALUES (?, ?)")
            .bind(issue_id)
            .bind(label)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

async fn write_custom_values(
    tx: &mut SqliteConnection,
    issue_id: i64,
    values: &[(i64, Option<String>)],
) -> sqlx::Result<()> {
    for (field_id, value) in values {
        match value {
            Some(value) => {
                sqlx::query(
                    "INSERT INTO issue_field_values (issue_id, field_id, value) VALUES (?, ?, ?)
                     ON CONFLICT (issue_id, field_id) DO UPDATE SET value = excluded.value",
                )
                .bind(issue_id)
                .bind(field_id)
                .bind(value)
                .execute(&mut *tx)
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM issue_field_values WHERE issue_id = ? AND field_id = ?")
                    .bind(issue_id)
                    .bind(field_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    Ok(())
}

/// Finds an issue by numeric id or by key, the two forms Jira accepts in URLs.
pub async fn by_id_or_key(db: &SqlitePool, id_or_key: &str) -> sqlx::Result<Option<Issue>> {
    let issue = match id_or_key.parse::<i64>() {
        Ok(id) => by_id(db, id).await?,
        Err(_) => {
            sqlx::query_as(concat!(
                "SELECT ",
                columns!(),
                " FROM issues WHERE key = ? COLLATE NOCASE"
            ))
            .bind(id_or_key)
            .fetch_optional(db)
            .await?
        }
    };
    Ok(issue)
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Issue>> {
    sqlx::query_as(concat!("SELECT ", columns!(), " FROM issues WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn by_ids(db: &SqlitePool, ids: &[i64]) -> sqlx::Result<Vec<Issue>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    // Safe: the only dynamic part is a list of `?` placeholders; the ids are bound.
    let sql = format!(
        concat!("SELECT ", columns!(), " FROM issues WHERE id IN ({})"),
        placeholders
    );
    let mut query = sqlx::query_as(sqlx::AssertSqlSafe(sql));
    for id in ids {
        query = query.bind(id);
    }
    let mut issues: Vec<Issue> = query.fetch_all(db).await?;
    // Keep the caller's order, which is the search order.
    issues.sort_by_key(|issue| ids.iter().position(|id| *id == issue.id));
    Ok(issues)
}

pub async fn labels(db: &SqlitePool, issue_id: i64) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar("SELECT label FROM issue_labels WHERE issue_id = ? ORDER BY label")
        .bind(issue_id)
        .fetch_all(db)
        .await
}

pub async fn subtasks(db: &SqlitePool, parent_id: i64) -> sqlx::Result<Vec<Issue>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM issues WHERE parent_id = ? ORDER BY id"
    ))
    .bind(parent_id)
    .fetch_all(db)
    .await
}

/// Custom field values of an issue as `(field id, JSON value)`.
pub async fn custom_values(db: &SqlitePool, issue_id: i64) -> sqlx::Result<Vec<(i64, String)>> {
    sqlx::query_as("SELECT field_id, value FROM issue_field_values WHERE issue_id = ?")
        .bind(issue_id)
        .fetch_all(db)
        .await
}

pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM issues WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Marks an issue as updated now, for example after someone comments on it.
pub async fn touch(db: &SqlitePool, issue_id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE issues SET updated_at = ? WHERE id = ?")
        .bind(now_ms())
        .bind(issue_id)
        .execute(db)
        .await?;
    Ok(())
}
