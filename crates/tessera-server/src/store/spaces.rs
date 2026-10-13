//! Confluence spaces.

use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Space {
    pub id: i64,
    pub key: String,
    pub name: String,
    pub description: String,
    pub space_type: String,
    pub status: String,
    pub homepage_id: Option<i64>,
    pub creator_id: i64,
    pub created_at: i64,
}

macro_rules! columns {
    () => {
        "id, key, name, description, type AS space_type, status, homepage_id, creator_id, created_at"
    };
}

pub struct NewSpace<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub space_type: &'a str,
    pub creator_id: i64,
    pub now: i64,
}

pub async fn create(db: &SqlitePool, space: &NewSpace<'_>) -> sqlx::Result<Space> {
    sqlx::query_as(concat!(
        "INSERT INTO spaces (key, name, description, type, creator_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING ",
        columns!()
    ))
    .bind(space.key)
    .bind(space.name)
    .bind(space.description)
    .bind(space.space_type)
    .bind(space.creator_id)
    .bind(space.now)
    .fetch_one(db)
    .await
}

pub async fn by_key(db: &SqlitePool, key: &str) -> sqlx::Result<Option<Space>> {
    sqlx::query_as(concat!("SELECT ", columns!(), " FROM spaces WHERE key = ?"))
        .bind(key)
        .fetch_optional(db)
        .await
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Space>> {
    sqlx::query_as(concat!("SELECT ", columns!(), " FROM spaces WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

/// Which spaces a listing returns. Empty lists mean "any".
#[derive(Default)]
pub struct SpaceFilter {
    pub keys: Vec<String>,
    pub ids: Vec<i64>,
    pub space_type: Option<String>,
    pub status: Option<String>,
}

/// One page of spaces, ordered by key as Confluence does, and whether more follow.
pub async fn list(
    db: &SqlitePool,
    filter: &SpaceFilter,
    start: i64,
    limit: i64,
) -> sqlx::Result<(Vec<Space>, bool)> {
    let mut spaces: Vec<Space> = sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM spaces ORDER BY key COLLATE NOCASE"
    ))
    .fetch_all(db)
    .await?;
    spaces.retain(|s| {
        (filter.keys.is_empty() || filter.keys.iter().any(|k| k.eq_ignore_ascii_case(&s.key)))
            && (filter.ids.is_empty() || filter.ids.contains(&s.id))
            && filter
                .space_type
                .as_ref()
                .is_none_or(|t| *t == s.space_type)
            && filter.status.as_ref().is_none_or(|t| *t == s.status)
    });
    let more = spaces.len() as i64 > start + limit;
    let page = spaces
        .into_iter()
        .skip(start as usize)
        .take(limit as usize)
        .collect();
    Ok((page, more))
}

pub async fn update(
    db: &SqlitePool,
    id: i64,
    name: &str,
    description: &str,
    status: &str,
    homepage_id: Option<i64>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE spaces SET name = ?, description = ?, status = ?, homepage_id = ? WHERE id = ?",
    )
    .bind(name)
    .bind(description)
    .bind(status)
    .bind(homepage_id)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

/// Deletes a space with all its content.
pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    // Content refers to itself, so clear those references before the cascade removes it.
    sqlx::query("UPDATE spaces SET homepage_id = NULL WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE content SET parent_id = NULL WHERE space_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM spaces WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}
