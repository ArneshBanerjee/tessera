//! Users. Each user has an opaque `account_id`, which is the id the Jira Cloud API exposes.

use sqlx::SqlitePool;

use crate::time::now_ms;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub account_id: String,
    pub email: String,
    pub display_name: String,
    pub password_hash: Option<String>,
    pub time_zone: String,
    pub active: bool,
    pub is_admin: bool,
}

/// The columns every query of this table selects, for `concat!` into static SQL.
macro_rules! columns {
    () => {
        "id, account_id, email, display_name, password_hash, time_zone, active, is_admin"
    };
}

pub struct NewUser<'a> {
    pub email: &'a str,
    pub display_name: &'a str,
    pub password_hash: Option<String>,
    pub is_admin: bool,
}

/// Creates a user. They join Confluence's default group, as new Atlassian users do.
pub async fn create(db: &SqlitePool, user: NewUser<'_>) -> sqlx::Result<User> {
    // Jira Cloud account ids are opaque strings; a random hex id is enough.
    let account_id = uuid::Uuid::new_v4().simple().to_string();
    let created: User = sqlx::query_as(concat!(
        "INSERT INTO users (account_id, email, display_name, password_hash, is_admin, created_at)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING ",
        columns!()
    ))
    .bind(account_id)
    .bind(user.email)
    .bind(user.display_name)
    .bind(user.password_hash)
    .bind(user.is_admin)
    .bind(now_ms())
    .fetch_one(db)
    .await?;
    sqlx::query("INSERT OR IGNORE INTO group_members (group_id, user_id) SELECT id, ? FROM groups WHERE name = ?")
        .bind(created.id)
        .bind(super::space_permissions::DEFAULT_GROUP)
        .execute(db)
        .await?;
    Ok(created)
}

pub async fn by_id(db: &SqlitePool, id: i64) -> sqlx::Result<Option<User>> {
    sqlx::query_as(concat!("SELECT ", columns!(), " FROM users WHERE id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn by_account_id(db: &SqlitePool, account_id: &str) -> sqlx::Result<Option<User>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM users WHERE account_id = ?"
    ))
    .bind(account_id)
    .fetch_optional(db)
    .await
}

pub async fn by_email(db: &SqlitePool, email: &str) -> sqlx::Result<Option<User>> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM users WHERE email = ?"
    ))
    .bind(email)
    .fetch_optional(db)
    .await
}

/// Every user. The JQL compiler uses this to resolve names, emails, and account ids.
pub async fn all(db: &SqlitePool) -> sqlx::Result<Vec<User>> {
    sqlx::query_as(concat!("SELECT ", columns!(), " FROM users ORDER BY id"))
        .fetch_all(db)
        .await
}

pub async fn count(db: &SqlitePool) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(db)
        .await
}

/// Finds active users whose name or email contains `query`, or whose account id equals it.
/// An empty query lists everyone.
pub async fn search(
    db: &SqlitePool,
    query: &str,
    start_at: i64,
    max_results: i64,
) -> sqlx::Result<Vec<User>> {
    let pattern = format!("%{}%", escape_like(query));
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM users
         WHERE active = 1 AND (display_name LIKE ?1 ESCAPE '\\' OR email LIKE ?1 ESCAPE '\\' OR account_id = ?4)
         ORDER BY display_name COLLATE NOCASE, id LIMIT ?2 OFFSET ?3"
    ))
    .bind(pattern)
    .bind(max_results)
    .bind(start_at)
    .bind(query)
    .fetch_all(db)
    .await
}

/// Escapes `%`, `_`, and `\` so text can be used inside a LIKE pattern.
pub fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub async fn update_profile(
    db: &SqlitePool,
    id: i64,
    display_name: &str,
    time_zone: &str,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET display_name = ?, time_zone = ? WHERE id = ?")
        .bind(display_name)
        .bind(time_zone)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn set_password_hash(db: &SqlitePool, id: i64, hash: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Sets whether a user can sign in, and whether they administer the site.
pub async fn set_flags(db: &SqlitePool, id: i64, active: bool, is_admin: bool) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET active = ?, is_admin = ? WHERE id = ?")
        .bind(active)
        .bind(is_admin)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// How many active administrators the site has, so the last one is never removed.
pub async fn active_admin_count(db: &SqlitePool) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE active = 1 AND is_admin = 1")
        .fetch_one(db)
        .await
}
