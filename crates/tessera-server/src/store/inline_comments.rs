//! Inline comments: what text they are on, and whether they are resolved.

use sqlx::SqlitePool;

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Inline {
    pub content_id: i64,
    pub marker_ref: String,
    pub original_selection: String,
    pub resolution: String,
    pub resolution_by: Option<i64>,
    pub resolution_at: Option<i64>,
}

impl Inline {
    /// The resolution as Confluence reports it: `dangling` when the marked text is gone from
    /// the page and the comment is not resolved.
    pub fn status(&self, page_body: &str) -> &str {
        let marked = page_body.contains(&format!("ac:ref=\"{}\"", self.marker_ref));
        if !marked && self.resolution != "resolved" {
            "dangling"
        } else {
            &self.resolution
        }
    }
}

pub async fn create(
    db: &SqlitePool,
    content_id: i64,
    marker_ref: &str,
    original_selection: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO inline_comments (content_id, marker_ref, original_selection) VALUES (?, ?, ?)",
    )
    .bind(content_id)
    .bind(marker_ref)
    .bind(original_selection)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn get(db: &SqlitePool, content_id: i64) -> sqlx::Result<Option<Inline>> {
    sqlx::query_as(
        "SELECT content_id, marker_ref, original_selection, resolution, resolution_by, resolution_at
         FROM inline_comments WHERE content_id = ?",
    )
    .bind(content_id)
    .fetch_optional(db)
    .await
}

/// Resolves or reopens a comment and its replies.
pub async fn set_resolved(
    db: &SqlitePool,
    marker_ref: &str,
    resolved: bool,
    user_id: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE inline_comments SET resolution = ?, resolution_by = ?, resolution_at = ?
         WHERE marker_ref = ?",
    )
    .bind(if resolved { "resolved" } else { "reopened" })
    .bind(user_id)
    .bind(now_ms())
    .bind(marker_ref)
    .execute(db)
    .await?;
    Ok(())
}
