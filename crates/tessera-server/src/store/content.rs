//! Confluence content: pages, blog posts, and comments, with every version kept.

use sqlx::{SqliteConnection, SqlitePool};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Content {
    pub id: i64,
    pub content_type: String,
    pub status: String,
    pub space_id: i64,
    pub parent_id: Option<i64>,
    pub container_id: Option<i64>,
    pub title: String,
    pub body: String,
    pub version: i64,
    pub position: i64,
    pub creator_id: i64,
    pub created_at: i64,
    pub updated_by: i64,
    pub updated_at: i64,
}

/// Columns of `content c`, for `concat!` into static SQL.
macro_rules! columns {
    () => {
        "c.id, c.type AS content_type, c.status, c.space_id, c.parent_id, c.container_id, c.title, \
         c.body, c.version, c.position, c.creator_id, c.created_at, c.updated_by, c.updated_at"
    };
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Version {
    pub content_id: i64,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub author_id: i64,
    pub created_at: i64,
    pub message: String,
    pub minor_edit: bool,
}

pub struct NewContent<'a> {
    pub content_type: &'a str,
    pub status: &'a str,
    pub space_id: i64,
    pub parent_id: Option<i64>,
    pub container_id: Option<i64>,
    pub title: &'a str,
    pub body: &'a str,
    pub author_id: i64,
    pub now: i64,
}

/// A plain-text copy of a storage-format body, for search. Code in macros counts as text.
pub fn plain_text(body: &str) -> String {
    let text = tessera_adf::to_plain_text(&tessera_adf::html::from_storage(body));
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub async fn create(db: &SqlitePool, new: &NewContent<'_>) -> sqlx::Result<Content> {
    let mut tx = db.begin().await?;
    // New pages go after their siblings.
    let position: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM content
         WHERE space_id = ? AND type = ? AND parent_id IS ?",
    )
    .bind(new.space_id)
    .bind(new.content_type)
    .bind(new.parent_id)
    .fetch_one(&mut *tx)
    .await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO content (type, status, space_id, parent_id, container_id, title, body, body_text,
                              position, creator_id, created_at, updated_by, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(new.content_type)
    .bind(new.status)
    .bind(new.space_id)
    .bind(new.parent_id)
    .bind(new.container_id)
    .bind(new.title)
    .bind(new.body)
    .bind(plain_text(new.body))
    .bind(position)
    .bind(new.author_id)
    .bind(new.now)
    .bind(new.author_id)
    .bind(new.now)
    .fetch_one(&mut *tx)
    .await?;
    insert_version(
        &mut tx,
        id,
        1,
        new.title,
        new.body,
        new.author_id,
        new.now,
        "",
        false,
    )
    .await?;
    tx.commit().await?;
    Ok(by_id(db, id).await?.expect("content was just created"))
}

#[allow(clippy::too_many_arguments)]
async fn insert_version(
    conn: &mut SqliteConnection,
    content_id: i64,
    number: i64,
    title: &str,
    body: &str,
    author_id: i64,
    now: i64,
    message: &str,
    minor_edit: bool,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO content_versions (content_id, number, title, body, author_id, created_at, message, minor_edit)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(content_id)
    .bind(number)
    .bind(title)
    .bind(body)
    .bind(author_id)
    .bind(now)
    .bind(message)
    .bind(minor_edit)
    .execute(conn)
    .await?;
    Ok(())
}

pub struct Edit<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub status: &'a str,
    pub parent_id: Option<i64>,
    pub author_id: i64,
    pub now: i64,
    pub message: &'a str,
    pub minor_edit: bool,
}

/// Saves a new version of `content`.
pub async fn update(db: &SqlitePool, content: &Content, edit: &Edit<'_>) -> sqlx::Result<Content> {
    let mut tx = db.begin().await?;
    let number = content.version + 1;
    sqlx::query(
        "UPDATE content SET title = ?, body = ?, body_text = ?, status = ?, parent_id = ?, version = ?,
                            updated_by = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(edit.title)
    .bind(edit.body)
    .bind(plain_text(edit.body))
    .bind(edit.status)
    .bind(edit.parent_id)
    .bind(number)
    .bind(edit.author_id)
    .bind(edit.now)
    .bind(content.id)
    .execute(&mut *tx)
    .await?;
    insert_version(
        &mut tx,
        content.id,
        number,
        edit.title,
        edit.body,
        edit.author_id,
        edit.now,
        edit.message,
        edit.minor_edit,
    )
    .await?;
    tx.commit().await?;
    Ok(by_id(db, content.id).await?.expect("content exists"))
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Content>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM content c WHERE c.id = ?"
    ))
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Whether another current page (or blog post) in the space already has this title.
pub async fn title_taken(
    db: &SqlitePool,
    space_id: i64,
    content_type: &str,
    title: &str,
    except: Option<i64>,
) -> sqlx::Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM content
         WHERE space_id = ? AND type = ? AND title = ? AND status = 'current' AND id IS NOT ?",
    )
    .bind(space_id)
    .bind(content_type)
    .bind(title)
    .bind(except)
    .fetch_one(db)
    .await?;
    Ok(count > 0)
}

/// Direct children of a page (or replies to a comment), in tree order.
pub async fn children(
    db: &SqlitePool,
    parent_id: i64,
    content_type: &str,
) -> sqlx::Result<Vec<Content>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM content c WHERE c.parent_id = ? AND c.type = ? AND c.status = 'current'
          ORDER BY c.position, c.id"
    ))
    .bind(parent_id)
    .bind(content_type)
    .fetch_all(db)
    .await
}

/// Every page below `id`, depth first in tree order.
pub async fn descendants(
    db: &SqlitePool,
    id: i64,
    content_type: &str,
) -> sqlx::Result<Vec<Content>> {
    let mut out = Vec::new();
    // Reversed, so the first child is popped first.
    let mut stack: Vec<Content> = children(db, id, content_type)
        .await?
        .into_iter()
        .rev()
        .collect();
    while let Some(node) = stack.pop() {
        let kids = children(db, node.id, content_type).await?;
        out.push(node);
        stack.extend(kids.into_iter().rev());
    }
    Ok(out)
}

/// The pages above `content`, from the top of the tree down to its parent.
pub async fn ancestors(db: &SqlitePool, content: &Content) -> sqlx::Result<Vec<Content>> {
    let mut out = Vec::new();
    let mut next = content.parent_id;
    while let Some(id) = next {
        let Some(parent) = by_id(db, id).await? else {
            break;
        };
        next = parent.parent_id;
        out.push(parent);
        // A cycle cannot be created through the API, but never loop forever.
        if out.len() > 1000 {
            break;
        }
    }
    out.reverse();
    Ok(out)
}

/// Comments on a page or blog post, oldest first.
pub async fn comments(db: &SqlitePool, container_id: i64) -> sqlx::Result<Vec<Content>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM content c WHERE c.container_id = ? AND c.type = 'comment' AND c.status = 'current'
          ORDER BY c.created_at, c.id"
    ))
    .bind(container_id)
    .fetch_all(db)
    .await
}

/// Top-level pages of a space, in tree order.
pub async fn root_pages(db: &SqlitePool, space_id: i64) -> sqlx::Result<Vec<Content>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM content c WHERE c.space_id = ? AND c.type = 'page' AND c.parent_id IS NULL
          AND c.status = 'current' ORDER BY c.position, c.id"
    ))
    .bind(space_id)
    .fetch_all(db)
    .await
}

/// Moves content to the trash. Child pages move up to the trashed page's parent.
pub async fn trash(db: &SqlitePool, content: &Content, now: i64) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    if content.content_type == "page" {
        sqlx::query("UPDATE content SET parent_id = ? WHERE parent_id = ? AND type = 'page'")
            .bind(content.parent_id)
            .bind(content.id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE content SET status = 'trashed', updated_at = ? WHERE id = ?")
        .bind(now)
        .bind(content.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// Restores trashed content to the top of its space.
pub async fn restore(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE content SET status = 'current', parent_id = NULL WHERE id = ? AND type <> 'comment'")
        .bind(id)
        .execute(db)
        .await?;
    sqlx::query("UPDATE content SET status = 'current' WHERE id = ? AND type = 'comment'")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Deletes content for good, with its versions, comments, and labels.
pub async fn purge(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM content WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn versions(db: &SqlitePool, content_id: i64) -> sqlx::Result<Vec<Version>> {
    sqlx::query_as(
        "SELECT content_id, number, title, body, author_id, created_at, message, minor_edit
         FROM content_versions WHERE content_id = ? ORDER BY number DESC",
    )
    .bind(content_id)
    .fetch_all(db)
    .await
}

pub async fn version(
    db: &SqlitePool,
    content_id: i64,
    number: i64,
) -> sqlx::Result<Option<Version>> {
    sqlx::query_as(
        "SELECT content_id, number, title, body, author_id, created_at, message, minor_edit
         FROM content_versions WHERE content_id = ? AND number = ?",
    )
    .bind(content_id)
    .bind(number)
    .fetch_optional(db)
    .await
}

/// Users who saved a version, in the order of their first edit.
pub async fn contributors(db: &SqlitePool, content_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT author_id FROM content_versions WHERE content_id = ?
         GROUP BY author_id ORDER BY MIN(number)",
    )
    .bind(content_id)
    .fetch_all(db)
    .await
}

pub async fn set_parent(db: &SqlitePool, id: i64, parent_id: Option<i64>) -> sqlx::Result<()> {
    sqlx::query("UPDATE content SET parent_id = ? WHERE id = ?")
        .bind(parent_id)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Gives sibling pages the order of `siblings`.
pub async fn reorder(db: &SqlitePool, content: &Content, siblings: &[i64]) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    for (position, sibling) in siblings.iter().enumerate() {
        sqlx::query("UPDATE content SET position = ? WHERE id = ? AND space_id = ?")
            .bind(position as i64)
            .bind(sibling)
            .bind(content.space_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

/// Which content a listing returns. `None` means "any".
#[derive(Default)]
pub struct ListFilter {
    pub space_id: Option<i64>,
    pub content_type: Option<String>,
    /// Defaults to `current`.
    pub status: Option<String>,
    pub title: Option<String>,
    /// Created at or after this time, and before `created_before`.
    pub created_from: Option<i64>,
    pub created_before: Option<i64>,
}

/// Content matching `filter`, oldest first.
pub async fn list(db: &SqlitePool, filter: &ListFilter) -> sqlx::Result<Vec<Content>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM content c
          WHERE (? IS NULL OR c.space_id = ?)
            AND (? IS NULL OR c.type = ?)
            AND c.status = ?
            AND (? IS NULL OR c.title = ?)
            AND (? IS NULL OR c.created_at >= ?)
            AND (? IS NULL OR c.created_at < ?)
          ORDER BY c.id"
    ))
    .bind(filter.space_id)
    .bind(filter.space_id)
    .bind(&filter.content_type)
    .bind(&filter.content_type)
    .bind(filter.status.as_deref().unwrap_or("current"))
    .bind(&filter.title)
    .bind(&filter.title)
    .bind(filter.created_from)
    .bind(filter.created_from)
    .bind(filter.created_before)
    .bind(filter.created_before)
    .fetch_all(db)
    .await
}
