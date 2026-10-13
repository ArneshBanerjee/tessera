//! Project versions (releases), and the fix and affected versions of issues.

use sqlx::{SqliteConnection, SqlitePool};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Version {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub description: String,
    pub archived: bool,
    pub released: bool,
    pub start_date: Option<String>,
    pub release_date: Option<String>,
    pub sequence: i64,
}

macro_rules! columns {
    () => {
        "id, project_id, name, description, archived, released, start_date, release_date, sequence"
    };
}

/// Which issue field a version belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `fixVersions`: the release that fixes the issue.
    Fix,
    /// `versions`: the releases the issue affects.
    Affects,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fix => "fix",
            Self::Affects => "affects",
        }
    }
}

pub struct VersionFields {
    pub name: String,
    pub description: String,
    pub archived: bool,
    pub released: bool,
    pub start_date: Option<String>,
    pub release_date: Option<String>,
}

pub async fn create(
    db: &SqlitePool,
    project_id: i64,
    fields: &VersionFields,
) -> sqlx::Result<Version> {
    sqlx::query_as(concat!(
        "INSERT INTO versions (project_id, name, description, archived, released, start_date, release_date, sequence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
            (SELECT COALESCE(MAX(sequence), 0) + 1 FROM versions WHERE project_id = ?1))
         RETURNING ",
        columns!()
    ))
    .bind(project_id)
    .bind(&fields.name)
    .bind(&fields.description)
    .bind(fields.archived)
    .bind(fields.released)
    .bind(&fields.start_date)
    .bind(&fields.release_date)
    .fetch_one(db)
    .await
}

pub async fn update(db: &SqlitePool, id: i64, fields: &VersionFields) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE versions SET name = ?, description = ?, archived = ?, released = ?, start_date = ?, release_date = ?
         WHERE id = ?",
    )
    .bind(&fields.name)
    .bind(&fields.description)
    .bind(fields.archived)
    .bind(fields.released)
    .bind(&fields.start_date)
    .bind(&fields.release_date)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Version>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM versions WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn for_project(db: &SqlitePool, project_id: i64) -> sqlx::Result<Vec<Version>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM versions WHERE project_id = ? ORDER BY sequence"
    ))
    .bind(project_id)
    .fetch_all(db)
    .await
}

/// All versions, for resolving JQL values and functions.
pub async fn all(db: &SqlitePool) -> sqlx::Result<Vec<Version>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM versions ORDER BY project_id, sequence"
    ))
    .fetch_all(db)
    .await
}

/// Deletes a version. Its issues move to the given versions, if any.
pub async fn delete(
    db: &SqlitePool,
    id: i64,
    move_fix_to: Option<i64>,
    move_affects_to: Option<i64>,
) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    for (kind, target) in [(Kind::Fix, move_fix_to), (Kind::Affects, move_affects_to)] {
        if let Some(target) = target {
            sqlx::query(
                "INSERT OR IGNORE INTO issue_versions (issue_id, version_id, kind)
                 SELECT issue_id, ?, kind FROM issue_versions WHERE version_id = ? AND kind = ?",
            )
            .bind(target)
            .bind(id)
            .bind(kind.as_str())
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query("DELETE FROM versions WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// Puts a version at a new position (1-based) within its project.
pub async fn move_to(db: &SqlitePool, version: &Version, position: i64) -> sqlx::Result<()> {
    let mut ordered = for_project(db, version.project_id).await?;
    ordered.retain(|v| v.id != version.id);
    let index = (position.max(1) as usize - 1).min(ordered.len());
    ordered.insert(index, version.clone());
    let mut tx = db.begin().await?;
    for (i, v) in ordered.iter().enumerate() {
        sqlx::query("UPDATE versions SET sequence = ? WHERE id = ?")
            .bind(i as i64 + 1)
            .bind(v.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

/// How many issues use a version as fix version and as affected version.
pub async fn issue_counts(db: &SqlitePool, id: i64) -> sqlx::Result<(i64, i64)> {
    sqlx::query_as(
        "SELECT
            COALESCE(SUM(kind = 'fix'), 0),
            COALESCE(SUM(kind = 'affects'), 0)
         FROM issue_versions WHERE version_id = ?",
    )
    .bind(id)
    .fetch_one(db)
    .await
}

/// Issues with this fix version that are not resolved.
pub async fn unresolved_count(db: &SqlitePool, id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM issue_versions v JOIN issues i ON i.id = v.issue_id
         WHERE v.version_id = ? AND v.kind = 'fix' AND i.resolution_id IS NULL",
    )
    .bind(id)
    .fetch_one(db)
    .await
}

/// The version ids of an issue for one field.
pub async fn of_issue(db: &SqlitePool, issue_id: i64, kind: Kind) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT v.version_id FROM issue_versions v JOIN versions ver ON ver.id = v.version_id
         WHERE v.issue_id = ? AND v.kind = ? ORDER BY ver.sequence",
    )
    .bind(issue_id)
    .bind(kind.as_str())
    .fetch_all(db)
    .await
}

pub async fn set_for_issue(
    conn: &mut SqliteConnection,
    issue_id: i64,
    kind: Kind,
    ids: &[i64],
) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM issue_versions WHERE issue_id = ? AND kind = ?")
        .bind(issue_id)
        .bind(kind.as_str())
        .execute(&mut *conn)
        .await?;
    for id in ids {
        sqlx::query(
            "INSERT OR IGNORE INTO issue_versions (issue_id, version_id, kind) VALUES (?, ?, ?)",
        )
        .bind(issue_id)
        .bind(id)
        .bind(kind.as_str())
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}
