//! Dashboards, their gadgets, and gadget settings (item properties).
//!
//! Gadgets sit in columns. Within a column, `row` orders them from the top without gaps.

use sqlx::{SqliteConnection, SqlitePool};

use super::filters::Share;
use crate::time::now_ms;

/// The dashboard everyone sees until they pick another one.
pub const SYSTEM_DASHBOARD: i64 = 10000;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Dashboard {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub owner_id: Option<i64>,
    pub layout: String,
    pub refresh_ms: Option<i64>,
}

impl Dashboard {
    pub fn is_system(&self) -> bool {
        self.owner_id.is_none()
    }

    /// How many columns the layout has.
    pub fn columns(&self) -> i64 {
        self.layout.len().max(1) as i64
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Gadget {
    pub id: i64,
    pub dashboard_id: i64,
    pub module_key: Option<String>,
    pub uri: Option<String>,
    pub title: String,
    pub color: String,
    pub col: i64,
    pub row: i64,
}

macro_rules! columns {
    () => {
        "id, name, description, owner_id, layout, refresh_ms"
    };
}

macro_rules! gadget_columns {
    () => {
        "id, dashboard_id, module_key, uri, title, color, col, row"
    };
}

pub async fn create(
    db: &SqlitePool,
    owner_id: i64,
    name: &str,
    description: &str,
) -> sqlx::Result<Dashboard> {
    sqlx::query_as(concat!(
        "INSERT INTO dashboards (name, description, owner_id, created_at) VALUES (?, ?, ?, ?) RETURNING ",
        columns!()
    ))
    .bind(name)
    .bind(description)
    .bind(owner_id)
    .bind(now_ms())
    .fetch_one(db)
    .await
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<Dashboard>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM dashboards WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Every dashboard, the system dashboard first. Callers decide which ones a user may see.
pub async fn all(db: &SqlitePool) -> sqlx::Result<Vec<Dashboard>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM dashboards ORDER BY owner_id IS NOT NULL, name COLLATE NOCASE, id"
    ))
    .fetch_all(db)
    .await
}

pub async fn update(db: &SqlitePool, dashboard: &Dashboard) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE dashboards SET name = ?, description = ?, layout = ?, refresh_ms = ? WHERE id = ?",
    )
    .bind(&dashboard.name)
    .bind(&dashboard.description)
    .bind(&dashboard.layout)
    .bind(dashboard.refresh_ms)
    .bind(dashboard.id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn delete(db: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM dashboards WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn shares(db: &SqlitePool, dashboard_id: i64) -> sqlx::Result<Vec<Share>> {
    sqlx::query_as(
        "SELECT share_type, project_id FROM dashboard_shares WHERE dashboard_id = ? ORDER BY id",
    )
    .bind(dashboard_id)
    .fetch_all(db)
    .await
}

pub async fn set_shares(db: &SqlitePool, dashboard_id: i64, shares: &[Share]) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM dashboard_shares WHERE dashboard_id = ?")
        .bind(dashboard_id)
        .execute(&mut *tx)
        .await?;
    for share in shares {
        sqlx::query(
            "INSERT INTO dashboard_shares (dashboard_id, share_type, project_id) VALUES (?, ?, ?)",
        )
        .bind(dashboard_id)
        .bind(&share.share_type)
        .bind(share.project_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

pub async fn set_favourite(
    db: &SqlitePool,
    dashboard_id: i64,
    user_id: i64,
    favourite: bool,
) -> sqlx::Result<()> {
    let sql = if favourite {
        "INSERT OR IGNORE INTO dashboard_favourites (dashboard_id, user_id) VALUES (?, ?)"
    } else {
        "DELETE FROM dashboard_favourites WHERE dashboard_id = ? AND user_id = ?"
    };
    sqlx::query(sql)
        .bind(dashboard_id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Whether `user_id` marked the dashboard as a favourite, and how many people did.
pub async fn favourite_state(
    db: &SqlitePool,
    dashboard_id: i64,
    user_id: i64,
) -> sqlx::Result<(bool, i64)> {
    sqlx::query_as(
        "SELECT COALESCE(SUM(user_id = ?), 0) > 0, COUNT(*) FROM dashboard_favourites
         WHERE dashboard_id = ?",
    )
    .bind(user_id)
    .bind(dashboard_id)
    .fetch_one(db)
    .await
}

pub async fn gadgets(db: &SqlitePool, dashboard_id: i64) -> sqlx::Result<Vec<Gadget>> {
    sqlx::query_as(concat!(
        "SELECT ",
        gadget_columns!(),
        " FROM dashboard_gadgets WHERE dashboard_id = ? ORDER BY col, row, id"
    ))
    .bind(dashboard_id)
    .fetch_all(db)
    .await
}

pub async fn gadget(
    db: &SqlitePool,
    dashboard_id: i64,
    gadget_id: i64,
) -> sqlx::Result<Option<Gadget>> {
    sqlx::query_as(concat!(
        "SELECT ",
        gadget_columns!(),
        " FROM dashboard_gadgets WHERE dashboard_id = ? AND id = ?"
    ))
    .bind(dashboard_id)
    .bind(gadget_id)
    .fetch_optional(db)
    .await
}

/// The number of gadgets in a column.
async fn column_size(
    conn: &mut SqliteConnection,
    dashboard_id: i64,
    col: i64,
) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM dashboard_gadgets WHERE dashboard_id = ? AND col = ?")
        .bind(dashboard_id)
        .bind(col)
        .fetch_one(conn)
        .await
}

/// Moves the gadgets at or below `row` in a column down by `by` (up when negative).
async fn shift(
    conn: &mut SqliteConnection,
    dashboard_id: i64,
    col: i64,
    row: i64,
    by: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE dashboard_gadgets SET row = row + ? WHERE dashboard_id = ? AND col = ? AND row >= ?",
    )
    .bind(by)
    .bind(dashboard_id)
    .bind(col)
    .bind(row)
    .execute(conn)
    .await?;
    Ok(())
}

pub struct NewGadget<'a> {
    pub module_key: Option<&'a str>,
    pub uri: Option<&'a str>,
    pub title: &'a str,
    pub color: &'a str,
    pub col: i64,
    /// Where in the column; the top when `None`, as Jira adds new gadgets.
    pub row: Option<i64>,
}

pub async fn add_gadget(
    db: &SqlitePool,
    dashboard_id: i64,
    gadget: NewGadget<'_>,
) -> sqlx::Result<Gadget> {
    let mut tx = db.begin().await?;
    let size = column_size(&mut tx, dashboard_id, gadget.col).await?;
    let row = gadget.row.unwrap_or(0).clamp(0, size);
    shift(&mut tx, dashboard_id, gadget.col, row, 1).await?;
    let added: Gadget = sqlx::query_as(concat!(
        "INSERT INTO dashboard_gadgets (dashboard_id, module_key, uri, title, color, col, row)
         VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING ",
        gadget_columns!()
    ))
    .bind(dashboard_id)
    .bind(gadget.module_key)
    .bind(gadget.uri)
    .bind(gadget.title)
    .bind(gadget.color)
    .bind(gadget.col)
    .bind(row)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(added)
}

/// Saves a gadget's title and color, and moves it to `(col, row)`.
pub async fn update_gadget(
    db: &SqlitePool,
    gadget: &Gadget,
    title: &str,
    color: &str,
    col: i64,
    row: i64,
) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    if (col, row) != (gadget.col, gadget.row) {
        // Take it out of its column, then make room at the new place.
        sqlx::query("UPDATE dashboard_gadgets SET col = -1 WHERE id = ?")
            .bind(gadget.id)
            .execute(&mut *tx)
            .await?;
        shift(&mut tx, gadget.dashboard_id, gadget.col, gadget.row + 1, -1).await?;
        let size = column_size(&mut tx, gadget.dashboard_id, col).await?;
        let row = row.clamp(0, size);
        shift(&mut tx, gadget.dashboard_id, col, row, 1).await?;
        sqlx::query("UPDATE dashboard_gadgets SET col = ?, row = ? WHERE id = ?")
            .bind(col)
            .bind(row)
            .bind(gadget.id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE dashboard_gadgets SET title = ?, color = ? WHERE id = ?")
        .bind(title)
        .bind(color)
        .bind(gadget.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

pub async fn delete_gadget(db: &SqlitePool, gadget: &Gadget) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM dashboard_gadgets WHERE id = ?")
        .bind(gadget.id)
        .execute(&mut *tx)
        .await?;
    shift(&mut tx, gadget.dashboard_id, gadget.col, gadget.row + 1, -1).await?;
    tx.commit().await
}

/// Puts gadgets from columns a smaller layout no longer has at the end of its last column.
pub async fn fit_columns(db: &SqlitePool, dashboard_id: i64, columns: i64) -> sqlx::Result<()> {
    let last = columns - 1;
    let mut tx = db.begin().await?;
    let mut next = column_size(&mut tx, dashboard_id, last).await?;
    let outside: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM dashboard_gadgets WHERE dashboard_id = ? AND col > ? ORDER BY col, row",
    )
    .bind(dashboard_id)
    .bind(last)
    .fetch_all(&mut *tx)
    .await?;
    for id in outside {
        sqlx::query("UPDATE dashboard_gadgets SET col = ?, row = ? WHERE id = ?")
            .bind(last)
            .bind(next)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        next += 1;
    }
    tx.commit().await
}

pub async fn property_keys(db: &SqlitePool, gadget_id: i64) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar("SELECT key FROM dashboard_item_properties WHERE gadget_id = ? ORDER BY key")
        .bind(gadget_id)
        .fetch_all(db)
        .await
}

/// A property's value as JSON text.
pub async fn property(db: &SqlitePool, gadget_id: i64, key: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT value FROM dashboard_item_properties WHERE gadget_id = ? AND key = ?",
    )
    .bind(gadget_id)
    .bind(key)
    .fetch_optional(db)
    .await
}

/// Sets a property. Returns whether it is new.
pub async fn set_property(
    db: &SqlitePool,
    gadget_id: i64,
    key: &str,
    value: &str,
) -> sqlx::Result<bool> {
    let existed = property(db, gadget_id, key).await?.is_some();
    sqlx::query(
        "INSERT INTO dashboard_item_properties (gadget_id, key, value) VALUES (?, ?, ?)
         ON CONFLICT (gadget_id, key) DO UPDATE SET value = excluded.value",
    )
    .bind(gadget_id)
    .bind(key)
    .bind(value)
    .execute(db)
    .await?;
    Ok(!existed)
}

/// Deletes a property. Returns whether it was there.
pub async fn delete_property(db: &SqlitePool, gadget_id: i64, key: &str) -> sqlx::Result<bool> {
    let result =
        sqlx::query("DELETE FROM dashboard_item_properties WHERE gadget_id = ? AND key = ?")
            .bind(gadget_id)
            .bind(key)
            .execute(db)
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Copies a dashboard's layout, gadgets, and gadget settings to a new dashboard.
pub async fn copy_contents(db: &SqlitePool, from: &Dashboard, to: i64) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE dashboards SET layout = ?, refresh_ms = ? WHERE id = ?")
        .bind(&from.layout)
        .bind(from.refresh_ms)
        .bind(to)
        .execute(&mut *tx)
        .await?;
    let originals: Vec<Gadget> = sqlx::query_as(concat!(
        "SELECT ",
        gadget_columns!(),
        " FROM dashboard_gadgets WHERE dashboard_id = ? ORDER BY col, row"
    ))
    .bind(from.id)
    .fetch_all(&mut *tx)
    .await?;
    for gadget in originals {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO dashboard_gadgets (dashboard_id, module_key, uri, title, color, col, row)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(to)
        .bind(&gadget.module_key)
        .bind(&gadget.uri)
        .bind(&gadget.title)
        .bind(&gadget.color)
        .bind(gadget.col)
        .bind(gadget.row)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO dashboard_item_properties (gadget_id, key, value)
             SELECT ?, key, value FROM dashboard_item_properties WHERE gadget_id = ?",
        )
        .bind(id)
        .bind(gadget.id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}
