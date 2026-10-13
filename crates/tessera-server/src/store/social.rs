//! Watchers and votes.

use sqlx::SqlitePool;

/// Which list: people who watch an issue, or people who voted for it.
#[derive(Debug, Clone, Copy)]
pub enum List {
    Watchers,
    Votes,
}

impl List {
    // Table names are fixed strings, never user input.
    fn insert(self) -> &'static str {
        match self {
            Self::Watchers => "INSERT OR IGNORE INTO watchers (issue_id, user_id) VALUES (?, ?)",
            Self::Votes => "INSERT OR IGNORE INTO votes (issue_id, user_id) VALUES (?, ?)",
        }
    }

    fn delete(self) -> &'static str {
        match self {
            Self::Watchers => "DELETE FROM watchers WHERE issue_id = ? AND user_id = ?",
            Self::Votes => "DELETE FROM votes WHERE issue_id = ? AND user_id = ?",
        }
    }

    fn select(self) -> &'static str {
        match self {
            Self::Watchers => "SELECT user_id FROM watchers WHERE issue_id = ? ORDER BY user_id",
            Self::Votes => "SELECT user_id FROM votes WHERE issue_id = ? ORDER BY user_id",
        }
    }
}

pub async fn add(db: &SqlitePool, list: List, issue_id: i64, user_id: i64) -> sqlx::Result<()> {
    sqlx::query(list.insert())
        .bind(issue_id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn remove(db: &SqlitePool, list: List, issue_id: i64, user_id: i64) -> sqlx::Result<()> {
    sqlx::query(list.delete())
        .bind(issue_id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// The user ids on the list for an issue.
pub async fn members(db: &SqlitePool, list: List, issue_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(list.select())
        .bind(issue_id)
        .fetch_all(db)
        .await
}
