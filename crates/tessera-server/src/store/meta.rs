//! Issue metadata: issue types, statuses, priorities, and resolutions.
//!
//! These tables are small, so a request loads them all at once into a [`Catalog`].

use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct IssueType {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub hierarchy_level: i64,
}

impl IssueType {
    pub fn is_subtask(&self) -> bool {
        self.hierarchy_level < 0
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StatusCategory {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub color_name: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Status {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub category_id: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Priority {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub status_color: String,
    pub sequence: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Resolution {
    pub id: i64,
    pub name: String,
    pub description: String,
}

/// All issue metadata, loaded once per request.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub issue_types: Vec<IssueType>,
    pub status_categories: Vec<StatusCategory>,
    pub statuses: Vec<Status>,
    pub priorities: Vec<Priority>,
    pub resolutions: Vec<Resolution>,
}

impl Catalog {
    pub async fn load(db: &SqlitePool) -> sqlx::Result<Self> {
        Ok(Self {
            issue_types: sqlx::query_as("SELECT * FROM issue_types ORDER BY id")
                .fetch_all(db)
                .await?,
            status_categories: sqlx::query_as("SELECT * FROM status_categories ORDER BY id")
                .fetch_all(db)
                .await?,
            statuses: sqlx::query_as("SELECT * FROM statuses ORDER BY id")
                .fetch_all(db)
                .await?,
            priorities: sqlx::query_as("SELECT * FROM priorities ORDER BY sequence")
                .fetch_all(db)
                .await?,
            resolutions: sqlx::query_as("SELECT * FROM resolutions ORDER BY id")
                .fetch_all(db)
                .await?,
        })
    }

    pub fn issue_type(&self, id: i64) -> Option<&IssueType> {
        self.issue_types.iter().find(|t| t.id == id)
    }

    pub fn status(&self, id: i64) -> Option<&Status> {
        self.statuses.iter().find(|s| s.id == id)
    }

    pub fn status_category(&self, id: i64) -> Option<&StatusCategory> {
        self.status_categories.iter().find(|c| c.id == id)
    }

    pub fn priority(&self, id: i64) -> Option<&Priority> {
        self.priorities.iter().find(|p| p.id == id)
    }

    pub fn resolution(&self, id: i64) -> Option<&Resolution> {
        self.resolutions.iter().find(|r| r.id == id)
    }

    /// The id of the default priority for new issues (Medium in a new site).
    pub fn default_priority(&self) -> Option<i64> {
        self.priorities
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case("Medium"))
            .or(self.priorities.first())
            .map(|p| p.id)
    }

    /// The resolution set when an issue moves to a done status (Done in a new site).
    pub fn default_resolution(&self) -> Option<i64> {
        self.resolutions
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case("Done"))
            .or(self.resolutions.first())
            .map(|r| r.id)
    }

    pub fn is_done(&self, status_id: i64) -> bool {
        self.status(status_id)
            .and_then(|s| self.status_category(s.category_id))
            .is_some_and(|c| c.key == "done")
    }
}

/// Finds an item by `{"id": ...}` or `{"name": ...}`, the two ways Jira clients refer to metadata.
pub fn find_by_ref<'a, T>(
    items: &'a [T],
    reference: &serde_json::Value,
    id: impl Fn(&T) -> i64,
    name: impl Fn(&T) -> &str,
) -> Option<&'a T> {
    if let Some(wanted) = reference.get("id").and_then(json_id) {
        return items.iter().find(|item| id(item) == wanted);
    }
    let wanted = reference.get("name")?.as_str()?;
    items
        .iter()
        .find(|item| name(item).eq_ignore_ascii_case(wanted))
}

/// Jira ids are strings in JSON (`"10000"`), but clients sometimes send numbers.
pub fn json_id(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::String(text) => text.parse().ok(),
        serde_json::Value::Number(number) => number.as_i64(),
        _ => None,
    }
}

pub async fn create_status(
    db: &SqlitePool,
    name: &str,
    description: &str,
    category_id: i64,
) -> sqlx::Result<Status> {
    sqlx::query_as(
        "INSERT INTO statuses (name, description, category_id) VALUES (?, ?, ?)
         RETURNING id, name, description, category_id",
    )
    .bind(name)
    .bind(description)
    .bind(category_id)
    .fetch_one(db)
    .await
}

pub async fn update_status(
    db: &SqlitePool,
    id: i64,
    name: &str,
    description: &str,
    category_id: i64,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE statuses SET name = ?, description = ?, category_id = ? WHERE id = ?")
        .bind(name)
        .bind(description)
        .bind(category_id)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Whether a workflow or an issue uses the status.
pub async fn status_in_use(db: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM workflow_statuses WHERE status_id = ?1)
              + (SELECT COUNT(*) FROM issues WHERE status_id = ?1)",
    )
    .bind(id)
    .fetch_one(db)
    .await?;
    Ok(count > 0)
}

pub async fn delete_status(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM statuses WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}
