//! Issues, their labels, and their custom field values.

use sqlx::{SqliteConnection, SqlitePool};

use crate::store::components;
use crate::store::versions::{self, Kind};
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
    /// Time tracking estimates, in seconds.
    pub original_estimate: Option<i64>,
    pub remaining_estimate: Option<i64>,
    /// Who may see the issue, when not everyone who can browse the project may.
    pub security_level_id: Option<i64>,
}

/// The columns every query of this table selects, for `concat!` into static SQL.
macro_rules! columns {
    () => {
        "id, project_id, number, key, issue_type_id, summary, description, environment, \
     status_id, priority_id, resolution_id, resolution_date, assignee_id, reporter_id, creator_id, \
     parent_id, due_date, created_at, updated_at, original_estimate, remaining_estimate, \
     security_level_id"
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
    pub components: Vec<i64>,
    pub fix_versions: Vec<i64>,
    pub affects_versions: Vec<i64>,
    /// Seconds.
    pub original_estimate: Option<i64>,
    /// Seconds.
    pub remaining_estimate: Option<i64>,
    /// The open (future or active) sprint the issue is in. Closed sprints stay as history.
    pub open_sprint: Option<i64>,
    pub security_level_id: Option<i64>,
    /// Custom field id and its JSON value. `None` clears the value.
    pub custom_values: Vec<(i64, Option<String>)>,
}

impl IssueFields {
    /// The current state of an issue, including its labels, components, and versions.
    /// Custom values are left empty; callers that edit them load them separately.
    pub async fn load(db: &SqlitePool, issue: &Issue) -> sqlx::Result<Self> {
        Ok(Self {
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
            labels: labels(db, issue.id).await?,
            components: components::of_issue(db, issue.id).await?,
            fix_versions: versions::of_issue(db, issue.id, Kind::Fix).await?,
            affects_versions: versions::of_issue(db, issue.id, Kind::Affects).await?,
            original_estimate: issue.original_estimate,
            remaining_estimate: issue.remaining_estimate,
            open_sprint: open_sprint(db, issue.id).await?,
            security_level_id: issue.security_level_id,
            custom_values: Vec::new(),
        })
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
    // New issues go to the bottom of the global rank order.
    let last_rank: Option<String> = sqlx::query_scalar("SELECT MAX(lexo_rank) FROM issues")
        .fetch_one(&mut *tx)
        .await?;
    let rank = crate::store::ranks::after(last_rank.as_deref());
    let issue: Issue = sqlx::query_as(concat!("INSERT INTO issues (project_id, number, key, issue_type_id, summary, description, environment,
            description_text, environment_text, status_id, priority_id, resolution_id, resolution_date,
            assignee_id, reporter_id, creator_id, parent_id, due_date, created_at, updated_at,
            original_estimate, remaining_estimate, lexo_rank, security_level_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
    .bind(fields.original_estimate)
    .bind(fields.remaining_estimate)
    .bind(&rank)
    .bind(fields.security_level_id)
    .fetch_one(&mut *tx)
    .await?;
    write_relations(&mut tx, issue.id, fields).await?;
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
            due_date = ?, original_estimate = ?, remaining_estimate = ?, security_level_id = ?,
            updated_at = ?
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
    .bind(fields.original_estimate)
    .bind(fields.remaining_estimate)
    .bind(fields.security_level_id)
    .bind(now)
    .bind(issue_id)
    .execute(&mut *tx)
    .await?;
    write_relations(&mut tx, issue_id, fields).await?;
    if !changes.is_empty() {
        crate::store::changelog::record(&mut tx, issue_id, author_id, now, changes).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Writes the list-valued fields: labels, components, versions, and custom values.
async fn write_relations(
    tx: &mut SqliteConnection,
    issue_id: i64,
    fields: &IssueFields,
) -> sqlx::Result<()> {
    replace_labels(tx, issue_id, &fields.labels).await?;
    components::set_for_issue(tx, issue_id, &fields.components).await?;
    versions::set_for_issue(tx, issue_id, Kind::Fix, &fields.fix_versions).await?;
    versions::set_for_issue(tx, issue_id, Kind::Affects, &fields.affects_versions).await?;
    set_open_sprint(tx, issue_id, fields.open_sprint).await?;
    write_custom_values(tx, issue_id, &fields.custom_values).await
}

/// The open sprint an issue is in, if any.
pub async fn open_sprint(db: &SqlitePool, issue_id: i64) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar(
        "SELECT s.id FROM issue_sprints i JOIN sprints s ON s.id = i.sprint_id
         WHERE i.issue_id = ? AND s.state != 'closed' ORDER BY s.sequence LIMIT 1",
    )
    .bind(issue_id)
    .fetch_optional(db)
    .await
}

/// Puts the issue in `sprint` (or no open sprint), keeping its closed sprints as history.
async fn set_open_sprint(
    conn: &mut SqliteConnection,
    issue_id: i64,
    sprint: Option<i64>,
) -> sqlx::Result<()> {
    sqlx::query(
        "DELETE FROM issue_sprints WHERE issue_id = ?
         AND sprint_id IN (SELECT id FROM sprints WHERE state != 'closed')",
    )
    .bind(issue_id)
    .execute(&mut *conn)
    .await?;
    if let Some(sprint_id) = sprint {
        sqlx::query("INSERT OR IGNORE INTO issue_sprints (issue_id, sprint_id) VALUES (?, ?)")
            .bind(issue_id)
            .bind(sprint_id)
            .execute(&mut *conn)
            .await?;
    }
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
    if issue.is_some() || id_or_key.parse::<i64>().is_ok() {
        return Ok(issue);
    }
    // A key the issue had before it moved to another project still finds it.
    let moved: Option<i64> = sqlx::query_scalar(
        "SELECT issue_id FROM moved_issue_keys WHERE old_key = ? COLLATE NOCASE",
    )
    .bind(id_or_key)
    .fetch_optional(db)
    .await?;
    match moved {
        Some(id) => by_id(db, id).await,
        None => Ok(None),
    }
}

/// Where a moved issue goes.
pub struct Relocation {
    pub project_id: i64,
    pub issue_type_id: i64,
    pub status_id: i64,
    pub security_level_id: Option<i64>,
}

/// Moves an issue to another project: it gets the project's next key, its old key keeps
/// finding it, and it leaves the old project's components, versions, and open sprints.
/// `changes` go in its history, with the change of key. Returns the new key.
pub async fn relocate(
    db: &SqlitePool,
    issue: &Issue,
    author_id: i64,
    to: &Relocation,
    changes: &[ChangeItem],
) -> sqlx::Result<String> {
    let mut tx = db.begin().await?;
    let mut key = issue.key.clone();
    let mut changes = changes.to_vec();
    if to.project_id != issue.project_id {
        let (number, project_key): (i64, String) = sqlx::query_as(
            "UPDATE projects SET issue_counter = issue_counter + 1 WHERE id = ? RETURNING issue_counter, key",
        )
        .bind(to.project_id)
        .fetch_one(&mut *tx)
        .await?;
        key = format!("{project_key}-{number}");
        sqlx::query("INSERT OR REPLACE INTO moved_issue_keys (old_key, issue_id) VALUES (?, ?)")
            .bind(&issue.key)
            .bind(issue.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE issues SET project_id = ?, number = ?, key = ? WHERE id = ?")
            .bind(to.project_id)
            .bind(number)
            .bind(&key)
            .bind(issue.id)
            .execute(&mut *tx)
            .await?;
        for table in ["issue_components", "issue_versions"] {
            // Safe: the table names are fixed above.
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DELETE FROM {table} WHERE issue_id = ?"
            )))
            .bind(issue.id)
            .execute(&mut *tx)
            .await?;
        }
        set_open_sprint(&mut tx, issue.id, None).await?;
        changes.push(ChangeItem {
            field: "Key".into(),
            field_id: "issuekey".into(),
            field_type: "jira".into(),
            from_value: None,
            from_string: Some(issue.key.clone()),
            to_value: None,
            to_string: Some(key.clone()),
        });
    }
    sqlx::query(
        "UPDATE issues SET issue_type_id = ?, status_id = ?, security_level_id = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(to.issue_type_id)
    .bind(to.status_id)
    .bind(to.security_level_id)
    .bind(now_ms())
    .bind(issue.id)
    .execute(&mut *tx)
    .await?;
    if !changes.is_empty() {
        crate::store::changelog::record(&mut tx, issue.id, author_id, now_ms(), &changes).await?;
    }
    tx.commit().await?;
    Ok(key)
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

/// Moves the project's issues of one type from one status to another, as when the project
/// changes workflows. Returns how many issues moved.
pub async fn move_status(
    db: &SqlitePool,
    project_id: i64,
    issue_type_id: i64,
    from_status_id: i64,
    to_status_id: i64,
) -> sqlx::Result<u64> {
    let result = sqlx::query(
        "UPDATE issues SET status_id = ? WHERE project_id = ? AND issue_type_id = ? AND status_id = ?",
    )
    .bind(to_status_id)
    .bind(project_id)
    .bind(issue_type_id)
    .bind(from_status_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}

/// The statuses that the project's issues of one type are in.
pub async fn statuses_in_use(
    db: &SqlitePool,
    project_id: i64,
    issue_type_id: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT DISTINCT status_id FROM issues WHERE project_id = ? AND issue_type_id = ? ORDER BY status_id",
    )
    .bind(project_id)
    .bind(issue_type_id)
    .fetch_all(db)
    .await
}

/// Gives an issue's subtasks its security level, which they always share.
pub async fn set_subtask_security(
    db: &SqlitePool,
    parent_id: i64,
    level: Option<i64>,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE issues SET security_level_id = ? WHERE parent_id = ?")
        .bind(level)
        .bind(parent_id)
        .execute(db)
        .await?;
    Ok(())
}
