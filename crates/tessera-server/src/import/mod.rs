//! Importing Atlassian backups: a Jira or Confluence backup zip (or its `entities.xml`) into
//! Tessera, keeping keys, ids that people see, dates, authors, and attachments.
//!
//! - [`jira`]: Jira's OfBiz entity XML.
//! - [`confluence`]: Confluence's Hibernate object XML.

pub mod confluence;
pub mod jira;
mod xml;

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use sqlx::SqliteConnection;

use crate::time::now_ms;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("could not read the backup: {0}")]
    Io(#[from] std::io::Error),
    #[error("the backup is not a valid zip file: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("the backup's XML is not valid: {0}")]
    Xml(String),
    #[error("the backup has no entities.xml")]
    NoEntities,
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

pub type ImportResult<T> = Result<T, ImportError>;

/// A backup: a zip from Jira or Confluence, or the `entities.xml` taken out of one.
pub struct Backup {
    path: PathBuf,
    zip: Option<zip::ZipArchive<File>>,
    /// Zip entry names by their last path segment, for finding attachment files.
    by_last_segment: HashMap<String, Vec<String>>,
}

impl Backup {
    pub fn open(path: &Path) -> ImportResult<Backup> {
        let is_zip = {
            let mut magic = [0u8; 4];
            let mut file = File::open(path)?;
            file.read(&mut magic)? == 4 && magic == [0x50, 0x4b, 0x03, 0x04]
        };
        let zip = if is_zip {
            Some(zip::ZipArchive::new(File::open(path)?)?)
        } else {
            None
        };
        let mut by_last_segment: HashMap<String, Vec<String>> = HashMap::new();
        if let Some(zip) = &zip {
            for name in zip.file_names() {
                let name = name?.to_string();
                let last = name.rsplit('/').next().unwrap_or(&name).to_string();
                by_last_segment.entry(last).or_default().push(name);
            }
        }
        Ok(Backup {
            path: path.to_path_buf(),
            zip,
            by_last_segment,
        })
    }

    /// Reads `entities.xml`, streaming.
    fn entities(&mut self) -> ImportResult<Box<dyn BufRead + '_>> {
        match &mut self.zip {
            None => Ok(Box::new(BufReader::new(File::open(&self.path)?))),
            Some(zip) => {
                let name = zip
                    .file_names()
                    .filter_map(Result::ok)
                    .find(|n| n.rsplit('/').next() == Some("entities.xml"))
                    .map(|n| n.to_string())
                    .ok_or(ImportError::NoEntities)?;
                Ok(Box::new(BufReader::new(zip.by_name(&name)?)))
            }
        }
    }

    /// The zip entries whose last path segment is `name`.
    fn entries_named(&self, name: &str) -> &[String] {
        self.by_last_segment
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The bytes of one zip entry.
    fn read(&mut self, entry: &str) -> ImportResult<Vec<u8>> {
        let zip = self.zip.as_mut().ok_or(ImportError::NoEntities)?;
        let mut file = zip.by_name(entry)?;
        let mut bytes = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn is_zip(&self) -> bool {
        self.zip.is_some()
    }
}

/// What an import brought in, for the person running it.
#[derive(Debug, Default)]
pub struct Summary {
    pub counts: Vec<(String, usize)>,
    pub notes: Vec<String>,
}

impl Summary {
    fn count(&mut self, what: &str, n: usize) {
        self.counts.push((what.to_string(), n));
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (what, n) in &self.counts {
            writeln!(f, "  {what}: {n}")?;
        }
        for note in &self.notes {
            writeln!(f, "Note: {note}")?;
        }
        Ok(())
    }
}

/// Milliseconds since the epoch for a backup's date-time, such as `2024-03-01 10:20:30.5`, an
/// ISO 8601 string, or milliseconds. Times without a zone are read as UTC.
pub fn parse_time(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(ms) = text.parse::<i64>() {
        return Some(ms);
    }
    if let Ok(stamp) = text.parse::<jiff::Timestamp>() {
        return Some(stamp.as_millisecond());
    }
    let civil = text.replace('T', " ");
    for format in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d",
    ] {
        if let Ok(time) = jiff::civil::DateTime::strptime(format, &civil) {
            return time
                .to_zoned(jiff::tz::TimeZone::UTC)
                .ok()
                .map(|z| z.timestamp().as_millisecond());
        }
        if let Ok(date) = jiff::civil::Date::strptime(format, &civil) {
            return date
                .to_zoned(jiff::tz::TimeZone::UTC)
                .ok()
                .map(|z| z.timestamp().as_millisecond());
        }
    }
    None
}

/// Whether an Atlassian user key looks like a Cloud account id, which Tessera keeps.
fn is_account_id(key: &str) -> bool {
    (key.len() >= 20 && key.chars().all(|c| c.is_ascii_hexdigit()))
        || key
            .split_once(':')
            .is_some_and(|(org, rest)| org.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty())
}

/// A person from a backup.
#[derive(Debug, Clone, Default)]
pub struct ImportedUser {
    /// The key other records name them by.
    pub key: String,
    pub name: String,
    pub display_name: String,
    pub email: String,
    pub active: bool,
}

/// Finds or creates the Tessera user for a person from a backup. People match by email;
/// someone without one gets a placeholder address. They have no password until they reset it
/// or an administrator sets one.
async fn user_for(conn: &mut SqliteConnection, person: &ImportedUser) -> ImportResult<i64> {
    let email = if person.email.contains('@') {
        person.email.trim().to_string()
    } else {
        let local: String = person
            .name
            .chars()
            .chain(person.key.chars())
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            .take(64)
            .collect();
        format!(
            "{}@imported.invalid",
            if local.is_empty() { "user" } else { &local }
        )
    };
    if let Some(id) =
        sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE email = ? COLLATE NOCASE")
            .bind(&email)
            .fetch_optional(&mut *conn)
            .await?
    {
        return Ok(id);
    }
    let account_id = if is_account_id(&person.key) {
        let taken: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE account_id = ?")
            .bind(&person.key)
            .fetch_optional(&mut *conn)
            .await?;
        if let Some(id) = taken {
            return Ok(id);
        }
        person.key.clone()
    } else {
        uuid::Uuid::new_v4().simple().to_string()
    };
    let display = if person.display_name.trim().is_empty() {
        if person.name.is_empty() {
            person.key.clone()
        } else {
            person.name.clone()
        }
    } else {
        person.display_name.clone()
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users (account_id, email, display_name, active, created_at)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&account_id)
    .bind(&email)
    .bind(display)
    .bind(person.active)
    .bind(now_ms())
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query("INSERT OR IGNORE INTO group_members (group_id, user_id) SELECT id, ? FROM groups WHERE name = ?")
        .bind(id)
        .bind(crate::store::space_permissions::DEFAULT_GROUP)
        .execute(&mut *conn)
        .await?;
    Ok(id)
}

/// Finds or creates a group by name, and returns its id.
async fn group_for(conn: &mut SqliteConnection, name: &str) -> ImportResult<i64> {
    if let Some(id) =
        sqlx::query_scalar::<_, i64>("SELECT id FROM groups WHERE name = ? COLLATE NOCASE")
            .bind(name)
            .fetch_optional(&mut *conn)
            .await?
    {
        return Ok(id);
    }
    Ok(
        sqlx::query_scalar("INSERT INTO groups (group_id, name) VALUES (?, ?) RETURNING id")
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(name)
            .fetch_one(&mut *conn)
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_backup_times() {
        assert_eq!(parse_time("1970-01-01 00:00:01.5"), Some(1500));
        assert_eq!(parse_time("1970-01-02"), Some(86_400_000));
        assert_eq!(parse_time("2024-01-01T00:00:00Z"), Some(1_704_067_200_000));
        assert_eq!(parse_time("1234"), Some(1234));
        assert_eq!(parse_time(""), None);
    }

    #[test]
    fn spots_cloud_account_ids() {
        assert!(is_account_id("5b10ac8d82e05b22cc7d4ef5"));
        assert!(is_account_id("557058:f58131cb-b67d-43c7-b30d-6b58d40bd077"));
        assert!(!is_account_id("JIRAUSER10000"));
        assert!(!is_account_id("admin"));
    }
}
