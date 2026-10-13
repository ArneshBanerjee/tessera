//! Agile details of epics: display name, color, and done flag.
//! An epic is any issue whose type has hierarchy level 1; this table only holds overrides.

use sqlx::SqlitePool;

#[derive(Debug, Clone, Default, sqlx::FromRow)]
pub struct EpicDetails {
    /// Falls back to the summary when unset.
    pub name: Option<String>,
    pub color_key: Option<String>,
    /// Falls back to the status category when unset.
    pub done: Option<bool>,
}

pub async fn details(db: &SqlitePool, issue_id: i64) -> sqlx::Result<EpicDetails> {
    Ok(
        sqlx::query_as("SELECT name, color_key, done FROM epics WHERE issue_id = ?")
            .bind(issue_id)
            .fetch_optional(db)
            .await?
            .unwrap_or_default(),
    )
}

pub async fn save(db: &SqlitePool, issue_id: i64, details: &EpicDetails) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO epics (issue_id, name, color_key, done) VALUES (?, ?, COALESCE(?, 'color_1'), ?)
         ON CONFLICT (issue_id) DO UPDATE SET
            name = excluded.name, color_key = excluded.color_key, done = excluded.done",
    )
    .bind(issue_id)
    .bind(&details.name)
    .bind(&details.color_key)
    .bind(details.done)
    .execute(db)
    .await?;
    Ok(())
}
