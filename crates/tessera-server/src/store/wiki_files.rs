//! The files of Confluence attachments. Each version of an attachment is one file, stored in
//! the data directory as `wiki-attachments/{content id}/{version}`.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use super::attachments::SaveError;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FileVersion {
    pub content_id: i64,
    pub version: i64,
    pub media_type: String,
    pub size: i64,
    pub comment: String,
}

pub fn dir(data_dir: &Path, content_id: i64) -> PathBuf {
    data_dir
        .join("wiki-attachments")
        .join(content_id.to_string())
}

pub fn path(data_dir: &Path, content_id: i64, version: i64) -> PathBuf {
    dir(data_dir, content_id).join(version.to_string())
}

/// Writes the file of one version of an attachment and records it. The row and the file are
/// saved together: if the row cannot be committed, the file is removed.
pub async fn save(
    db: &SqlitePool,
    data_dir: &Path,
    file: &FileVersion,
    bytes: &[u8],
) -> Result<(), SaveError> {
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO attachment_files (content_id, version, media_type, size, comment)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(file.content_id)
    .bind(file.version)
    .bind(&file.media_type)
    .bind(file.size)
    .bind(&file.comment)
    .execute(&mut *tx)
    .await?;
    let target = path(data_dir, file.content_id, file.version);
    tokio::fs::create_dir_all(dir(data_dir, file.content_id)).await?;
    tokio::fs::write(&target, bytes).await?;
    if let Err(error) = tx.commit().await {
        let _ = tokio::fs::remove_file(&target).await;
        return Err(error.into());
    }
    Ok(())
}

/// The file of a version, or of the latest version when `version` is `None`.
pub async fn find(
    db: &SqlitePool,
    content_id: i64,
    version: Option<i64>,
) -> sqlx::Result<Option<FileVersion>> {
    sqlx::query_as(
        "SELECT content_id, version, media_type, size, comment FROM attachment_files
         WHERE content_id = ? AND (? IS NULL OR version = ?)
         ORDER BY version DESC LIMIT 1",
    )
    .bind(content_id)
    .bind(version)
    .bind(version)
    .fetch_optional(db)
    .await
}

/// Removes every file of an attachment. Missing files are fine.
pub async fn remove_all(data_dir: &Path, content_id: i64) {
    let _ = tokio::fs::remove_dir_all(dir(data_dir, content_id)).await;
}
