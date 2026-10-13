//! Entity properties: JSON values that apps attach to boards and sprints by key.

use sqlx::SqlitePool;

/// The kinds of entities that carry properties.
#[derive(Debug, Clone, Copy)]
pub enum Entity {
    Board,
    Sprint,
}

impl Entity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::Sprint => "sprint",
        }
    }
}

pub async fn keys(db: &SqlitePool, entity: Entity, id: i64) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT key FROM entity_properties WHERE entity_type = ? AND entity_id = ? ORDER BY key",
    )
    .bind(entity.as_str())
    .bind(id)
    .fetch_all(db)
    .await
}

pub async fn get(
    db: &SqlitePool,
    entity: Entity,
    id: i64,
    key: &str,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT value FROM entity_properties WHERE entity_type = ? AND entity_id = ? AND key = ?",
    )
    .bind(entity.as_str())
    .bind(id)
    .bind(key)
    .fetch_optional(db)
    .await
}

/// Sets a property. Returns true if it is new.
pub async fn set(
    db: &SqlitePool,
    entity: Entity,
    id: i64,
    key: &str,
    value: &str,
) -> sqlx::Result<bool> {
    let existed = get(db, entity, id, key).await?.is_some();
    sqlx::query(
        "INSERT INTO entity_properties (entity_type, entity_id, key, value) VALUES (?, ?, ?, ?)
         ON CONFLICT (entity_type, entity_id, key) DO UPDATE SET value = excluded.value",
    )
    .bind(entity.as_str())
    .bind(id)
    .bind(key)
    .bind(value)
    .execute(db)
    .await?;
    Ok(!existed)
}

/// Deletes a property. Returns false if it did not exist.
pub async fn delete(db: &SqlitePool, entity: Entity, id: i64, key: &str) -> sqlx::Result<bool> {
    Ok(sqlx::query(
        "DELETE FROM entity_properties WHERE entity_type = ? AND entity_id = ? AND key = ?",
    )
    .bind(entity.as_str())
    .bind(id)
    .bind(key)
    .execute(db)
    .await?
    .rows_affected()
        > 0)
}
