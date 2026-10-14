//! Workflows: which statuses a project uses and how issues move between them.

use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Transition {
    pub id: i64,
    pub name: String,
    pub to_status_id: i64,
}

/// The status where new issues in this workflow start.
pub async fn initial_status(db: &SqlitePool, workflow_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT status_id FROM workflow_statuses WHERE workflow_id = ? ORDER BY is_initial DESC, status_id LIMIT 1",
    )
    .bind(workflow_id)
    .fetch_one(db)
    .await
}

/// The statuses of a workflow, in their natural order.
pub async fn statuses(db: &SqlitePool, workflow_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT status_id FROM workflow_statuses WHERE workflow_id = ? ORDER BY status_id",
    )
    .bind(workflow_id)
    .fetch_all(db)
    .await
}

/// Transitions available from `status_id`: its own outgoing transitions plus global ones.
/// Like Jira, a transition that would leave the issue in the same status is not offered.
pub async fn available(
    db: &SqlitePool,
    workflow_id: i64,
    status_id: i64,
) -> sqlx::Result<Vec<Transition>> {
    sqlx::query_as(
        "SELECT id, name, to_status_id FROM workflow_transitions
         WHERE workflow_id = ?1 AND (from_status_id IS NULL OR from_status_id = ?2) AND to_status_id != ?2
         ORDER BY id",
    )
    .bind(workflow_id)
    .bind(status_id)
    .fetch_all(db)
    .await
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Workflow {
    pub id: i64,
    pub name: String,
    pub description: String,
}

/// A transition as stored. `from_status_id` is `None` for a global transition.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredTransition {
    pub id: i64,
    pub name: String,
    pub from_status_id: Option<i64>,
    pub to_status_id: i64,
}

/// A transition of a new workflow: from these statuses (none means from any status) to one.
pub struct NewTransition<'a> {
    pub name: &'a str,
    pub from: &'a [i64],
    pub to: i64,
}

pub async fn all(db: &SqlitePool) -> sqlx::Result<Vec<Workflow>> {
    sqlx::query_as("SELECT id, name, description FROM workflows ORDER BY name COLLATE NOCASE")
        .fetch_all(db)
        .await
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Workflow>> {
    sqlx::query_as("SELECT id, name, description FROM workflows WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn by_name(db: &SqlitePool, name: &str) -> sqlx::Result<Option<Workflow>> {
    sqlx::query_as("SELECT id, name, description FROM workflows WHERE name = ?")
        .bind(name)
        .fetch_optional(db)
        .await
}

pub async fn transitions(db: &SqlitePool, workflow_id: i64) -> sqlx::Result<Vec<StoredTransition>> {
    sqlx::query_as(
        "SELECT id, name, from_status_id, to_status_id FROM workflow_transitions
         WHERE workflow_id = ? ORDER BY id",
    )
    .bind(workflow_id)
    .fetch_all(db)
    .await
}

/// Creates a workflow with its statuses and transitions in one step. A transition with several
/// source statuses is stored once per source, each with its own id.
pub async fn create(
    db: &SqlitePool,
    name: &str,
    description: &str,
    statuses: &[i64],
    initial_status: i64,
    transitions: &[NewTransition<'_>],
) -> sqlx::Result<Workflow> {
    let mut tx = db.begin().await?;
    let workflow: Workflow = sqlx::query_as(
        "INSERT INTO workflows (name, description) VALUES (?, ?) RETURNING id, name, description",
    )
    .bind(name)
    .bind(description)
    .fetch_one(&mut *tx)
    .await?;
    for status in statuses {
        sqlx::query(
            "INSERT INTO workflow_statuses (workflow_id, status_id, is_initial) VALUES (?, ?, ?)",
        )
        .bind(workflow.id)
        .bind(status)
        .bind(*status == initial_status)
        .execute(&mut *tx)
        .await?;
    }
    // Transition ids count up in tens, like Jira's.
    let mut next: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM workflow_transitions")
        .fetch_one(&mut *tx)
        .await?;
    for transition in transitions {
        let sources: Vec<Option<i64>> = if transition.from.is_empty() {
            vec![None]
        } else {
            transition.from.iter().copied().map(Some).collect()
        };
        for from in sources {
            next = (next / 10 + 1) * 10 + 1;
            sqlx::query(
                "INSERT INTO workflow_transitions (id, workflow_id, name, from_status_id, to_status_id)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(next)
            .bind(workflow.id)
            .bind(transition.name)
            .bind(from)
            .bind(transition.to)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(workflow)
}

/// Whether any workflow scheme uses the workflow.
pub async fn in_use(db: &SqlitePool, workflow_id: i64) -> sqlx::Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM workflow_schemes WHERE default_workflow_id = ?1)
              + (SELECT COUNT(*) FROM workflow_scheme_mappings WHERE workflow_id = ?1)",
    )
    .bind(workflow_id)
    .fetch_one(db)
    .await?;
    Ok(count > 0)
}

pub async fn delete(db: &SqlitePool, workflow_id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM workflows WHERE id = ?")
        .bind(workflow_id)
        .execute(db)
        .await?;
    Ok(())
}

// Schemes.

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Scheme {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub default_workflow_id: i64,
}

pub const DEFAULT_SCHEME: i64 = 10000;

pub async fn schemes(db: &SqlitePool) -> sqlx::Result<Vec<Scheme>> {
    sqlx::query_as(
        "SELECT id, name, description, default_workflow_id FROM workflow_schemes ORDER BY id",
    )
    .fetch_all(db)
    .await
}

pub async fn scheme(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Scheme>> {
    sqlx::query_as(
        "SELECT id, name, description, default_workflow_id FROM workflow_schemes WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// The workflow for each issue type that does not use the default: (issue type, workflow).
pub async fn mappings(db: &SqlitePool, scheme_id: i64) -> sqlx::Result<Vec<(i64, i64)>> {
    sqlx::query_as(
        "SELECT issue_type_id, workflow_id FROM workflow_scheme_mappings WHERE scheme_id = ? ORDER BY issue_type_id",
    )
    .bind(scheme_id)
    .fetch_all(db)
    .await
}

/// Creates or replaces a scheme's settings: its name, description, default, and mappings.
pub async fn save_scheme(
    db: &SqlitePool,
    id: Option<i64>,
    name: &str,
    description: &str,
    default_workflow_id: i64,
    mappings: &[(i64, i64)],
) -> sqlx::Result<Scheme> {
    let mut tx = db.begin().await?;
    let scheme: Scheme = match id {
        Some(id) => sqlx::query_as(
            "UPDATE workflow_schemes SET name = ?, description = ?, default_workflow_id = ? WHERE id = ?
             RETURNING id, name, description, default_workflow_id",
        )
        .bind(name)
        .bind(description)
        .bind(default_workflow_id)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?,
        None => sqlx::query_as(
            "INSERT INTO workflow_schemes (name, description, default_workflow_id) VALUES (?, ?, ?)
             RETURNING id, name, description, default_workflow_id",
        )
        .bind(name)
        .bind(description)
        .bind(default_workflow_id)
        .fetch_one(&mut *tx)
        .await?,
    };
    sqlx::query("DELETE FROM workflow_scheme_mappings WHERE scheme_id = ?")
        .bind(scheme.id)
        .execute(&mut *tx)
        .await?;
    for (issue_type, workflow) in mappings {
        sqlx::query("INSERT INTO workflow_scheme_mappings (scheme_id, issue_type_id, workflow_id) VALUES (?, ?, ?)")
            .bind(scheme.id)
            .bind(issue_type)
            .bind(workflow)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(scheme)
}

pub async fn scheme_in_use(db: &SqlitePool, scheme_id: i64) -> sqlx::Result<bool> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE workflow_scheme_id = ?")
            .bind(scheme_id)
            .fetch_one(db)
            .await?;
    Ok(count > 0)
}

pub async fn delete_scheme(db: &SqlitePool, scheme_id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM workflow_schemes WHERE id = ?")
        .bind(scheme_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn project_scheme(db: &SqlitePool, project_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT workflow_scheme_id FROM projects WHERE id = ?")
        .bind(project_id)
        .fetch_one(db)
        .await
}

pub async fn set_project_scheme(
    db: &SqlitePool,
    project_id: i64,
    scheme_id: i64,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE projects SET workflow_scheme_id = ? WHERE id = ?")
        .bind(scheme_id)
        .bind(project_id)
        .execute(db)
        .await?;
    Ok(())
}

/// The workflow that issues of a type follow in a project.
pub async fn for_issue_type(
    db: &SqlitePool,
    project_id: i64,
    issue_type_id: i64,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COALESCE(m.workflow_id, s.default_workflow_id)
         FROM projects p
         JOIN workflow_schemes s ON s.id = p.workflow_scheme_id
         LEFT JOIN workflow_scheme_mappings m ON m.scheme_id = s.id AND m.issue_type_id = ?
         WHERE p.id = ?",
    )
    .bind(issue_type_id)
    .bind(project_id)
    .fetch_one(db)
    .await
}

/// Every status any workflow of the project's scheme uses, in their natural order.
pub async fn project_statuses(db: &SqlitePool, project_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT DISTINCT ws.status_id FROM workflow_statuses ws
         WHERE ws.workflow_id IN (
             SELECT s.default_workflow_id FROM projects p JOIN workflow_schemes s ON s.id = p.workflow_scheme_id WHERE p.id = ?1
             UNION
             SELECT m.workflow_id FROM projects p JOIN workflow_scheme_mappings m ON m.scheme_id = p.workflow_scheme_id WHERE p.id = ?1
         )
         ORDER BY ws.status_id",
    )
    .bind(project_id)
    .fetch_all(db)
    .await
}
