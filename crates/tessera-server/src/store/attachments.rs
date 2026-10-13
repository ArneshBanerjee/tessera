//! Attachment metadata. File contents are stored in the data directory, named by id.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Attachment {
    pub id: i64,
    pub issue_id: i64,
    pub author_id: i64,
    pub filename: String,
    pub mime_type: String,
    pub size: i64,
    pub created_at: i64,
}

macro_rules! columns {
    () => {
        "id, issue_id, author_id, filename, mime_type, size, created_at"
    };
}

/// Saving an attachment touches the database and the disk; either can fail.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    File(#[from] std::io::Error),
}

/// Where the contents of an attachment live.
pub fn path(data_dir: &Path, id: i64) -> PathBuf {
    data_dir.join("attachments").join(id.to_string())
}

/// Records the attachment and writes its file as one step: the row is inserted in a transaction,
/// the file is written, and then the transaction commits. If the commit fails, the file is removed.
pub async fn create(
    db: &SqlitePool,
    data_dir: &Path,
    issue_id: i64,
    author_id: i64,
    filename: &str,
    mime_type: &str,
    bytes: &[u8],
) -> Result<Attachment, SaveError> {
    let mut tx = db.begin().await?;
    let attachment: Attachment = sqlx::query_as(concat!(
        "INSERT INTO attachments (issue_id, author_id, filename, mime_type, size, created_at)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING ",
        columns!()
    ))
    .bind(issue_id)
    .bind(author_id)
    .bind(filename)
    .bind(mime_type)
    .bind(bytes.len() as i64)
    .bind(now_ms())
    .fetch_one(&mut *tx)
    .await?;
    let file = path(data_dir, attachment.id);
    tokio::fs::create_dir_all(file.parent().expect("attachments dir")).await?;
    tokio::fs::write(&file, bytes).await?;
    if let Err(error) = tx.commit().await {
        let _ = tokio::fs::remove_file(&file).await;
        return Err(error.into());
    }
    Ok(attachment)
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Attachment>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM attachments WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn of_issue(db: &SqlitePool, issue_id: i64) -> sqlx::Result<Vec<Attachment>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM attachments WHERE issue_id = ? ORDER BY created_at, id"
    ))
    .bind(issue_id)
    .fetch_all(db)
    .await
}

/// Removes the record and the file.
pub async fn delete(db: &SqlitePool, data_dir: &Path, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM attachments WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    // A missing file is fine: the record is what clients see.
    let _ = tokio::fs::remove_file(path(data_dir, id)).await;
    Ok(())
}
