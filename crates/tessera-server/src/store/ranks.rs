//! Issue ranking: the global order that boards and backlogs use.
//!
//! Ranks are strings over `0-9a-z` that sort as text. New issues go to the end: their rank is
//! the first ten characters of the current last rank, read as a base-36 number, plus a large
//! step. Moving an issue between two others gives it a rank between theirs, which only gets
//! longer when issues are packed closely. Only the moved issue changes.

use sqlx::SqlitePool;

const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
const WIDTH: usize = 10;
/// Space left between appended ranks, so many issues fit between two neighbors.
const STEP: u64 = 36u64.pow(4);

fn digit(c: u8) -> u64 {
    DIGITS.iter().position(|d| *d == c).unwrap_or(0) as u64
}

fn to_base36(mut value: u64) -> String {
    let mut out = vec![b'0'; WIDTH];
    for slot in out.iter_mut().rev() {
        *slot = DIGITS[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8(out).expect("ASCII digits")
}

/// The rank for an issue placed after `last` (or first of all, when there is none).
pub fn after(last: Option<&str>) -> String {
    let base = last
        .map(|rank| {
            rank.bytes()
                .chain(std::iter::repeat(b'0'))
                .take(WIDTH)
                .fold(0u64, |n, c| n * 36 + digit(c))
        })
        .unwrap_or(0);
    to_base36(base + STEP)
}

/// A rank strictly between `low` and `high`. `None` means no bound on that side.
pub fn between(low: Option<&str>, high: Option<&str>) -> String {
    // With no upper bound, any rank after `low` will do.
    let Some(high) = high else {
        return after(low);
    };
    let low = low.unwrap_or("").as_bytes();
    let mut high = Some(high.as_bytes());
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let lo = low.get(i).map_or(0, |c| digit(*c));
        let hi = match high {
            Some(h) => h.get(i).map_or(0, |c| digit(*c)),
            None => 36,
        };
        if hi > lo + 1 {
            out.push(DIGITS[((lo + hi) / 2) as usize]);
            break;
        }
        // Equal or adjacent digits: keep the low digit and look further. Once the low digit is
        // smaller, the high bound no longer limits the next positions.
        out.push(DIGITS[lo as usize]);
        if hi > lo {
            high = None;
        }
        i += 1;
        // Guard against invalid input (low >= high): never loop forever.
        if i > 256 {
            out.push(b'i');
            break;
        }
    }
    String::from_utf8(out).expect("ASCII digits")
}

/// The rank of the last issue, if any.
pub async fn last(db: &SqlitePool) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT MAX(lexo_rank) FROM issues")
        .fetch_one(db)
        .await
}

/// Gives ranks to issues that have none, in id order, after all ranked issues.
/// Runs at startup, so issues from before ranking existed get an order.
pub async fn backfill(db: &SqlitePool) -> sqlx::Result<()> {
    let unranked: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM issues WHERE lexo_rank IS NULL ORDER BY id")
            .fetch_all(db)
            .await?;
    let mut rank = last(db).await?;
    for id in unranked {
        let next = after(rank.as_deref());
        sqlx::query("UPDATE issues SET lexo_rank = ? WHERE id = ?")
            .bind(&next)
            .bind(id)
            .execute(db)
            .await?;
        rank = Some(next);
    }
    Ok(())
}

pub async fn of_issue(db: &SqlitePool, issue_id: i64) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT lexo_rank FROM issues WHERE id = ?")
        .bind(issue_id)
        .fetch_one(db)
        .await
}

/// Moves issues, in the given order, directly before or after `anchor`.
pub async fn place(
    db: &SqlitePool,
    issue_ids: &[i64],
    anchor: i64,
    before_anchor: bool,
) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    let anchor_rank: Option<String> =
        sqlx::query_scalar("SELECT lexo_rank FROM issues WHERE id = ?")
            .bind(anchor)
            .fetch_one(&mut *tx)
            .await?;
    let anchor_rank = anchor_rank.unwrap_or_else(|| after(None));
    // The neighbor on the other side of the anchor bounds the gap we fill. Moved issues are
    // left out, since they are leaving their places.
    let placeholders = vec!["?"; issue_ids.len().max(1)].join(", ");
    let neighbor_sql = if before_anchor {
        format!(
            "SELECT MAX(lexo_rank) FROM issues WHERE lexo_rank < ? AND id NOT IN ({placeholders})"
        )
    } else {
        format!(
            "SELECT MIN(lexo_rank) FROM issues WHERE lexo_rank > ? AND id NOT IN ({placeholders})"
        )
    };
    // Safe: the only dynamic part is a list of `?` placeholders.
    let mut query = sqlx::query_scalar(sqlx::AssertSqlSafe(neighbor_sql)).bind(&anchor_rank);
    for id in issue_ids.iter().chain(issue_ids.is_empty().then_some(&-1)) {
        query = query.bind(*id);
    }
    let neighbor: Option<String> = query.fetch_one(&mut *tx).await?;
    let (mut low, high) = if before_anchor {
        (neighbor, Some(anchor_rank))
    } else {
        (Some(anchor_rank), neighbor)
    };
    for id in issue_ids {
        let rank = between(low.as_deref(), high.as_deref());
        sqlx::query("UPDATE issues SET lexo_rank = ? WHERE id = ?")
            .bind(&rank)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        low = Some(rank);
    }
    tx.commit().await
}

/// How Jira shows a rank: `0|<rank>:`.
pub fn display(rank: &str) -> String {
    format!("0|{rank}:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appended_ranks_increase() {
        let first = after(None);
        let second = after(Some(&first));
        let third = after(Some(&second));
        assert!(first < second && second < third);
        assert_eq!(first.len(), WIDTH);
    }

    #[test]
    fn between_is_strictly_between() {
        let cases = [
            ("0000010000", "0000010001"),
            ("a", "b"),
            ("a", "a1"),
            ("0000000005", "zzzzzzzzzz"),
            ("hz", "i"),
        ];
        for (low, high) in cases {
            let mid = between(Some(low), Some(high));
            assert!(
                low < mid.as_str() && mid.as_str() < high,
                "{low} < {mid} < {high}"
            );
        }
        let top = after(None);
        let before_top = between(None, Some(&top));
        assert!(before_top < top);
    }

    #[test]
    fn repeated_inserts_stay_ordered() {
        let (low, mut high) = (after(None), after(Some(&after(None))));
        for _ in 0..200 {
            let mid = between(Some(&low), Some(&high));
            assert!(low < mid && mid < high);
            high = mid;
        }
        assert!(high.len() < 60, "ranks grow slowly: {}", high.len());
    }

    #[test]
    fn appending_after_a_long_rank_still_sorts_last() {
        let long = between(Some("0000010000"), Some("0000010001"));
        let next = after(Some(&long));
        assert!(long < next);
    }
}
