//! Issue history (changelog).

use sqlx::{SqliteConnection, SqlitePool};

use crate::store::issues::ChangeItem;

#[derive(Debug, Clone)]
pub struct Changelog {
    pub id: i64,
    pub author_id: i64,
    pub created_at: i64,
    pub items: Vec<ChangeItem>,
}

/// Records one history entry with all of its field changes.
pub async fn record(
    conn: &mut SqliteConnection,
    issue_id: i64,
    author_id: i64,
    created_at: i64,
    items: &[ChangeItem],
) -> sqlx::Result<()> {
    let changelog_id: i64 = sqlx::query_scalar(
        "INSERT INTO changelogs (issue_id, author_id, created_at) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(issue_id)
    .bind(author_id)
    .bind(created_at)
    .fetch_one(&mut *conn)
    .await?;
    for item in items {
        sqlx::query(
            "INSERT INTO changelog_items (changelog_id, field, field_id, field_type, from_value,
                from_string, to_value, to_string)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(changelog_id)
        .bind(&item.field)
        .bind(&item.field_id)
        .bind(&item.field_type)
        .bind(&item.from_value)
        .bind(&item.from_string)
        .bind(&item.to_value)
        .bind(&item.to_string)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// The history of an issue, oldest first.
pub async fn for_issue(db: &SqlitePool, issue_id: i64) -> sqlx::Result<Vec<Changelog>> {
    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT id, author_id, created_at FROM changelogs WHERE issue_id = ? ORDER BY id",
    )
    .bind(issue_id)
    .fetch_all(db)
    .await?;
    let mut changelogs = Vec::with_capacity(rows.len());
    for (id, author_id, created_at) in rows {
        let items = sqlx::query_as(
            "SELECT field, field_id, field_type, from_value, from_string, to_value, to_string
             FROM changelog_items WHERE changelog_id = ? ORDER BY id",
        )
        .bind(id)
        .fetch_all(db)
        .await?;
        changelogs.push(Changelog {
            id,
            author_id,
            created_at,
            items,
        });
    }
    Ok(changelogs)
}
