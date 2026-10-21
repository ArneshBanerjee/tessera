//! Importing a Confluence backup.
//!
//! Brings in users, spaces (with their home pages and permissions), pages and blog posts with
//! every version, the page tree, comments with replies, labels, and attachments with their
//! files. Bodies are already in storage format and go in as they are.
//!
//! A space whose key Tessera already has is skipped, with a note.

use std::collections::HashMap;
use std::path::Path;

use sqlx::{SqliteConnection, SqlitePool};

use super::xml::{Entity, read_hibernate};
use super::{Backup, ImportResult, ImportedUser, Summary, parse_time, user_for};
use crate::store::{content, space_permissions, wiki_files};
use crate::time::now_ms;

const KINDS: &[&str] = &[
    "ConfluenceUserImpl",
    "Space",
    "SpaceDescription",
    "Page",
    "BlogPost",
    "Comment",
    "BodyContent",
    "Attachment",
    "ContentProperty",
    "Labelling",
    "Label",
    "SpacePermission",
];

/// Confluence's space permission types, as Tessera's (operation, target).
fn operation_of(kind: &str) -> Option<(&'static str, &'static str)> {
    Some(match kind {
        "VIEWSPACE" => ("read", "space"),
        "EDITSPACE" => ("create", "page"),
        "EDITBLOG" => ("create", "blogpost"),
        "COMMENT" => ("create", "comment"),
        "CREATEATTACHMENT" => ("create", "attachment"),
        "REMOVEOWNCONTENT" => ("delete", "space"),
        "REMOVEPAGE" => ("delete", "page"),
        "REMOVEBLOG" => ("delete", "blogpost"),
        "REMOVECOMMENT" => ("delete", "comment"),
        "REMOVEATTACHMENT" => ("delete", "attachment"),
        "EXPORTSPACE" => ("export", "space"),
        "SETPAGEPERMISSIONS" => ("restrict_content", "space"),
        "ARCHIVEPAGE" => ("archive", "page"),
        "SETSPACEPERMISSIONS" => ("administer", "space"),
        _ => return None,
    })
}

#[derive(Default)]
struct Records(HashMap<String, Vec<Entity>>);

impl Records {
    fn of(&self, kind: &str) -> &[Entity] {
        self.0.get(kind).map(Vec::as_slice).unwrap_or_default()
    }
}

fn status_of(entity: &Entity) -> &'static str {
    match entity
        .get("contentStatus")
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("deleted") => "trashed",
        Some("draft") => "draft",
        _ => "current",
    }
}

/// Imports a Confluence backup in one transaction.
pub async fn import(db: &SqlitePool, data_dir: &Path, path: &Path) -> ImportResult<Summary> {
    let mut backup = Backup::open(path)?;
    let mut records = Records::default();
    read_hibernate(
        backup.entities()?,
        |k| KINDS.contains(&k),
        |entity| {
            records
                .0
                .entry(entity.kind.clone())
                .or_default()
                .push(entity);
            Ok(())
        },
    )?;
    let mut summary = Summary::default();
    let mut tx = db.begin().await?;
    let mut users: HashMap<String, i64> = HashMap::new();
    let mut accounts: HashMap<String, String> = HashMap::new();
    let mut people: HashMap<String, ImportedUser> = HashMap::new();
    for user in records.of("ConfluenceUserImpl") {
        let key = user.text("key");
        people.insert(
            key.clone(),
            ImportedUser {
                key: key.clone(),
                name: user.text("name"),
                display_name: user.text("name"),
                email: user.text("email"),
                active: true,
            },
        );
    }
    let conn: &mut SqliteConnection = &mut tx;
    let fallback: i64 = match sqlx::query_scalar::<_, i64>(
        "SELECT id FROM users WHERE is_admin = 1 ORDER BY id LIMIT 1",
    )
    .fetch_optional(&mut *conn)
    .await?
    {
        Some(id) => id,
        None => {
            user_for(
                conn,
                &ImportedUser {
                    key: "imported-user".into(),
                    name: "imported-user".into(),
                    ..Default::default()
                },
            )
            .await?
        }
    };
    for person in people.values() {
        let id = user_for(conn, person).await?;
        users.insert(person.key.clone(), id);
        let account: String = sqlx::query_scalar("SELECT account_id FROM users WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
        accounts.insert(person.key.clone(), account);
    }
    summary.count("users", users.len());
    let user = |key: &str| users.get(key).copied().unwrap_or(fallback);

    // Bodies by the content they belong to, and content properties.
    let mut bodies: HashMap<String, String> = HashMap::new();
    for body in records.of("BodyContent") {
        bodies.insert(body.text("content"), body.text("body"));
    }
    let mut properties: HashMap<(String, String), &Entity> = HashMap::new();
    for property in records.of("ContentProperty") {
        properties.insert((property.text("content"), property.text("name")), property);
    }

    // Spaces.
    let descriptions: HashMap<String, String> = records
        .of("SpaceDescription")
        .iter()
        .map(|d| {
            (
                d.text("id"),
                content::plain_text(
                    bodies
                        .get(&d.text("id"))
                        .map(String::as_str)
                        .unwrap_or_default(),
                ),
            )
        })
        .collect();
    let mut spaces: HashMap<String, (i64, String)> = HashMap::new();
    for space in records.of("Space") {
        let key = space.text("key");
        let taken: Option<i64> =
            sqlx::query_scalar("SELECT id FROM spaces WHERE key = ? COLLATE NOCASE")
                .bind(&key)
                .fetch_optional(&mut *conn)
                .await?;
        if taken.is_some() || key.is_empty() {
            summary.notes.push(format!(
                "Space {key} was skipped: Tessera already has a space with that key."
            ));
            continue;
        }
        let kind = if space
            .get("spaceType")
            .is_some_and(|t| t.eq_ignore_ascii_case("personal"))
        {
            "personal"
        } else {
            "global"
        };
        let status = if space
            .get("spaceStatus")
            .is_some_and(|s| s.eq_ignore_ascii_case("archived"))
        {
            "archived"
        } else {
            "current"
        };
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO spaces (key, name, description, type, status, creator_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&key)
        .bind(space.text("name"))
        .bind(
            descriptions
                .get(&space.text("description"))
                .cloned()
                .unwrap_or_default(),
        )
        .bind(kind)
        .bind(status)
        .bind(user(&space.text("creator")))
        .bind(
            space
                .get("creationDate")
                .and_then(parse_time)
                .unwrap_or_else(now_ms),
        )
        .fetch_one(&mut *conn)
        .await?;
        spaces.insert(space.text("id"), (id, space.text("homePage")));
    }
    summary.count("spaces", spaces.len());

    // Space permissions, or Tessera's defaults for spaces the backup gives none.
    let mut granted: HashMap<i64, usize> = HashMap::new();
    for permission in records.of("SpacePermission") {
        let (Some((space, _)), Some((operation, target))) = (
            spaces.get(&permission.text("space")),
            operation_of(&permission.text("type")),
        ) else {
            continue;
        };
        let (subject_type, subject) = if let Some(group) = permission.get("group") {
            ("group", group.to_string())
        } else if let Some(key) = permission.get("userSubject") {
            match accounts.get(key) {
                Some(account) => ("user", account.clone()),
                None => continue,
            }
        } else {
            ("anonymous", String::new())
        };
        if subject_type == "group" {
            super::group_for(conn, &subject).await?;
        }
        sqlx::query(
            "INSERT OR IGNORE INTO space_permissions (space_id, subject_type, subject, operation, target)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(space)
        .bind(subject_type)
        .bind(&subject)
        .bind(operation)
        .bind(target)
        .execute(&mut *conn)
        .await?;
        *granted.entry(*space).or_default() += 1;
    }
    for (space, _) in spaces.values() {
        if granted.contains_key(space) {
            continue;
        }
        for (operation, target) in space_permissions::GROUP_DEFAULTS {
            sqlx::query(
                "INSERT OR IGNORE INTO space_permissions (space_id, subject_type, subject, operation, target)
                 VALUES (?, 'group', ?, ?, ?)",
            )
            .bind(space)
            .bind(space_permissions::DEFAULT_GROUP)
            .bind(operation)
            .bind(target)
            .execute(&mut *conn)
            .await?;
        }
    }

    // Pages and blog posts. Old versions are records of their own that point at the current
    // one through `originalVersion`.
    let mut contents: HashMap<String, i64> = HashMap::new();
    let mut versions_of: HashMap<String, Vec<&Entity>> = HashMap::new();
    let mut currents: Vec<(&Entity, &str)> = Vec::new();
    for (kind, label) in [("Page", "page"), ("BlogPost", "blogpost")] {
        for item in records.of(kind) {
            match item.get("originalVersion") {
                Some(original) => versions_of
                    .entry(original.to_string())
                    .or_default()
                    .push(item),
                None => currents.push((item, label)),
            }
        }
    }
    let mut count = 0;
    for (item, kind) in &currents {
        let Some((space, _)) = spaces.get(&item.text("space")) else {
            continue;
        };
        let body = bodies.get(&item.text("id")).cloned().unwrap_or_default();
        let created = item
            .get("creationDate")
            .and_then(parse_time)
            .unwrap_or_else(now_ms);
        let updated = item
            .get("lastModificationDate")
            .and_then(parse_time)
            .unwrap_or(created);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO content (type, status, space_id, title, body, body_text, version, position,
                creator_id, created_at, updated_by, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(kind)
        .bind(status_of(item))
        .bind(space)
        .bind(item.text("title"))
        .bind(&body)
        .bind(content::plain_text(&body))
        .bind(item.int("version").unwrap_or(1))
        .bind(item.int("position").unwrap_or(0))
        .bind(user(&item.text("creator")))
        .bind(created)
        .bind(user(&item.text("lastModifier")))
        .bind(updated)
        .fetch_one(&mut *conn)
        .await?;
        contents.insert(item.text("id"), id);
        // Every version, the current one included.
        let mut history: Vec<&Entity> = versions_of
            .get(&item.text("id"))
            .cloned()
            .unwrap_or_default();
        history.push(item);
        for version in history {
            let body = bodies.get(&version.text("id")).cloned().unwrap_or_default();
            sqlx::query(
                "INSERT OR IGNORE INTO content_versions (content_id, number, title, body, author_id,
                    created_at, message, minor_edit)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 0)",
            )
            .bind(id)
            .bind(version.int("version").unwrap_or(1))
            .bind(version.text("title"))
            .bind(body)
            .bind(user(&version.text("lastModifier").or_else_text(version, "creator")))
            .bind(version.get("lastModificationDate").and_then(parse_time).unwrap_or(created))
            .bind(version.text("versionComment"))
            .execute(&mut *conn)
            .await?;
        }
        count += 1;
    }
    summary.count("pages and blog posts", count);
    // The page tree and home pages, now that every page has its new id.
    for (item, _) in &currents {
        if let (Some(id), Some(parent)) = (
            contents.get(&item.text("id")),
            item.get("parent").and_then(|p| contents.get(p)),
        ) {
            sqlx::query("UPDATE content SET parent_id = ? WHERE id = ?")
                .bind(parent)
                .bind(id)
                .execute(&mut *conn)
                .await?;
        }
    }
    for (space, home) in spaces.values() {
        if let Some(home) = contents.get(home) {
            sqlx::query("UPDATE spaces SET homepage_id = ? WHERE id = ?")
                .bind(home)
                .bind(space)
                .execute(&mut *conn)
                .await?;
        }
    }

    // Comments, then their replies.
    let comments: Vec<&Entity> = records
        .of("Comment")
        .iter()
        .filter(|c| c.get("originalVersion").is_none())
        .collect();
    let mut comment_count = 0;
    for comment in &comments {
        let container_key = comment
            .get("containerContent")
            .or_else(|| comment.get("page"))
            .unwrap_or_default();
        let Some(container) = contents.get(container_key) else {
            continue;
        };
        let space: i64 = sqlx::query_scalar("SELECT space_id FROM content WHERE id = ?")
            .bind(container)
            .fetch_one(&mut *conn)
            .await?;
        let title: String = sqlx::query_scalar("SELECT title FROM content WHERE id = ?")
            .bind(container)
            .fetch_one(&mut *conn)
            .await?;
        let body = bodies.get(&comment.text("id")).cloned().unwrap_or_default();
        let created = comment
            .get("creationDate")
            .and_then(parse_time)
            .unwrap_or_else(now_ms);
        let author = user(&comment.text("creator"));
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO content (type, status, space_id, container_id, title, body, body_text, version,
                creator_id, created_at, updated_by, updated_at)
             VALUES ('comment', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(status_of(comment))
        .bind(space)
        .bind(container)
        .bind(format!("Re: {title}"))
        .bind(&body)
        .bind(content::plain_text(&body))
        .bind(comment.int("version").unwrap_or(1))
        .bind(author)
        .bind(created)
        .bind(user(&comment.text("lastModifier").or_else_text(comment, "creator")))
        .bind(comment.get("lastModificationDate").and_then(parse_time).unwrap_or(created))
        .fetch_one(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO content_versions (content_id, number, title, body, author_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(comment.int("version").unwrap_or(1))
        .bind(format!("Re: {title}"))
        .bind(&body)
        .bind(author)
        .bind(created)
        .execute(&mut *conn)
        .await?;
        contents.insert(comment.text("id"), id);
        comment_count += 1;
    }
    for comment in &comments {
        if let (Some(id), Some(parent)) = (
            contents.get(&comment.text("id")),
            comment.get("parent").and_then(|p| contents.get(p)),
        ) {
            sqlx::query("UPDATE content SET parent_id = ? WHERE id = ?")
                .bind(parent)
                .bind(id)
                .execute(&mut *conn)
                .await?;
        }
    }
    summary.count("comments", comment_count);

    // Labels.
    let labels: HashMap<String, &Entity> = records
        .of("Label")
        .iter()
        .map(|l| (l.text("id"), l))
        .collect();
    let mut label_count = 0;
    for labelling in records.of("Labelling") {
        let (Some(target), Some(label)) = (
            contents.get(&labelling.text("content")),
            labels.get(&labelling.text("label")),
        ) else {
            continue;
        };
        let prefix = label
            .get("namespace")
            .filter(|n| matches!(*n, "my" | "team"))
            .unwrap_or("global");
        let label_id: i64 = sqlx::query_scalar(
            "INSERT INTO labels (prefix, name) VALUES (?, ?)
             ON CONFLICT (prefix, name) DO UPDATE SET name = excluded.name RETURNING id",
        )
        .bind(prefix)
        .bind(label.text("name"))
        .fetch_one(&mut *conn)
        .await?;
        sqlx::query("INSERT OR IGNORE INTO content_labels (content_id, label_id) VALUES (?, ?)")
            .bind(target)
            .bind(label_id)
            .execute(&mut *conn)
            .await?;
        label_count += 1;
    }
    summary.count("labels on content", label_count);

    // Attachments: a record per version; files at `attachments/<page>/<attachment>/<version>`.
    let mut attachment_versions: HashMap<String, Vec<&Entity>> = HashMap::new();
    let mut attachments: Vec<&Entity> = Vec::new();
    for attachment in records.of("Attachment") {
        match attachment.get("originalVersion") {
            Some(original) => attachment_versions
                .entry(original.to_string())
                .or_default()
                .push(attachment),
            None => attachments.push(attachment),
        }
    }
    // (Tessera id, version, source container id, source attachment id) for each file.
    let mut files: Vec<(i64, i64, String, String)> = Vec::new();
    for attachment in &attachments {
        let source_container = attachment
            .get("containerContent")
            .or_else(|| attachment.get("content"))
            .unwrap_or_default()
            .to_string();
        let Some(container) = contents.get(&source_container) else {
            continue;
        };
        let space: i64 = sqlx::query_scalar("SELECT space_id FROM content WHERE id = ?")
            .bind(container)
            .fetch_one(&mut *conn)
            .await?;
        let created = attachment
            .get("creationDate")
            .and_then(parse_time)
            .unwrap_or_else(now_ms);
        let version = attachment.int("version").unwrap_or(1);
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO content (type, status, space_id, container_id, title, body, body_text, version,
                creator_id, created_at, updated_by, updated_at)
             VALUES ('attachment', ?, ?, ?, ?, '', '', ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(status_of(attachment))
        .bind(space)
        .bind(container)
        .bind(attachment.text("title"))
        .bind(version)
        .bind(user(&attachment.text("creator")))
        .bind(created)
        .bind(user(&attachment.text("lastModifier").or_else_text(attachment, "creator")))
        .bind(attachment.get("lastModificationDate").and_then(parse_time).unwrap_or(created))
        .fetch_one(&mut *conn)
        .await?;
        let mut history: Vec<&Entity> = attachment_versions
            .get(&attachment.text("id"))
            .cloned()
            .unwrap_or_default();
        history.push(attachment);
        for each in history {
            let number = each.int("version").unwrap_or(1);
            let media_type = properties
                .get(&(each.text("id"), "MEDIA_TYPE".to_string()))
                .map(|p| p.text("stringValue"))
                .or_else(|| {
                    each.get("mediaType")
                        .or_else(|| each.get("contentType"))
                        .map(str::to_string)
                })
                .unwrap_or_else(|| {
                    crate::confluence::service::media_type(None, &each.text("title"))
                });
            let size = properties
                .get(&(each.text("id"), "FILESIZE".to_string()))
                .and_then(|p| p.int("longValue"))
                .or_else(|| each.int("fileSize"))
                .unwrap_or(0);
            let created = each
                .get("creationDate")
                .and_then(parse_time)
                .unwrap_or(created);
            sqlx::query(
                "INSERT OR IGNORE INTO content_versions (content_id, number, title, body, author_id, created_at, message)
                 VALUES (?, ?, ?, '', ?, ?, ?)",
            )
            .bind(id)
            .bind(number)
            .bind(each.text("title"))
            .bind(user(&each.text("creator")))
            .bind(each.get("lastModificationDate").and_then(parse_time).unwrap_or(created))
            .bind(each.text("versionComment"))
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "INSERT OR IGNORE INTO attachment_files (content_id, version, media_type, size, comment)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(number)
            .bind(media_type)
            .bind(size)
            .bind(each.text("versionComment"))
            .execute(&mut *conn)
            .await?;
            files.push((id, number, source_container.clone(), attachment.text("id")));
        }
    }
    summary.count("attachments", attachments.len());
    tx.commit().await?;

    let mut written = 0;
    let mut missing = 0;
    if backup.is_zip() {
        for (id, version, container, source) in files {
            // Newer backups keep version files under the attachment's folder; older ones flat.
            let candidates = backup.entries_named(&version.to_string());
            let entry = candidates
                .iter()
                .find(|n| n.contains(&format!("/{container}/{source}/")))
                .or_else(|| {
                    candidates
                        .iter()
                        .find(|n| n.contains(&format!("/{source}/")))
                })
                .cloned();
            match entry {
                Some(entry) => {
                    let bytes = backup.read(&entry)?;
                    let target = wiki_files::path(data_dir, id, version);
                    if let Some(dir) = target.parent() {
                        std::fs::create_dir_all(dir)?;
                    }
                    std::fs::write(target, bytes)?;
                    written += 1;
                }
                None => missing += 1,
            }
        }
    }
    summary.count("attachment files", written);
    if missing > 0 {
        summary.notes.push(format!(
            "{missing} attachment files were not in the backup."
        ));
    }
    Ok(summary)
}

/// A field, or another when it is empty.
trait OrElseText {
    fn or_else_text(self, entity: &Entity, other: &str) -> String;
}

impl OrElseText for String {
    fn or_else_text(self, entity: &Entity, other: &str) -> String {
        if self.is_empty() {
            entity.text(other)
        } else {
            self
        }
    }
}
