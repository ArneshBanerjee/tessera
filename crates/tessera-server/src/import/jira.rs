//! Importing a Jira backup.
//!
//! Brings in users and groups, issue types, statuses, priorities, resolutions, projects (with
//! their components and versions), and issues with their keys, descriptions, comments, labels,
//! links, subtasks and epics, watchers, votes, worklogs, history, custom field values, and
//! attachment files. Text in Jira wiki markup becomes ADF.
//!
//! A project whose key Tessera already has is skipped, with a note. Boards and sprints live
//! outside `entities.xml`, in Jira's add-on tables, and are not imported.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use super::xml::{Entity, read_ofbiz};
use super::{Backup, ImportResult, ImportedUser, Summary, group_for, parse_time, user_for};
use crate::store::ranks;
use crate::time::now_ms;

/// The record kinds the importer reads.
const KINDS: &[&str] = &[
    "User",
    "ApplicationUser",
    "Group",
    "Membership",
    "Project",
    "IssueType",
    "Status",
    "Priority",
    "Resolution",
    "Issue",
    "Action",
    "Label",
    "Component",
    "Version",
    "NodeAssociation",
    "UserAssociation",
    "Worklog",
    "IssueLink",
    "IssueLinkType",
    "ChangeGroup",
    "ChangeItem",
    "FileAttachment",
    "CustomField",
    "CustomFieldValue",
    "CustomFieldOption",
];

/// The records of a backup, by kind.
#[derive(Default)]
struct Records(HashMap<String, Vec<Entity>>);

impl Records {
    fn of(&self, kind: &str) -> &[Entity] {
        self.0.get(kind).map(Vec::as_slice).unwrap_or_default()
    }
}

/// Text in Jira wiki markup as ADF JSON, and its plain text for search.
fn rich(markup: &str) -> (Option<String>, String) {
    if markup.trim().is_empty() {
        return (None, String::new());
    }
    let doc = tessera_adf::wiki::from_wiki_markup(markup);
    let text = tessera_adf::to_plain_text(&doc);
    (Some(serde_json::to_string(&doc).unwrap_or_default()), text)
}

/// Imports a Jira backup. Everything goes in one transaction: a failed import changes nothing.
pub async fn import(db: &SqlitePool, data_dir: &Path, path: &Path) -> ImportResult<Summary> {
    let mut backup = Backup::open(path)?;
    let mut records = Records::default();
    read_ofbiz(
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
    let mut tx = db.begin().await?;
    let mut importer = Importer {
        records: &records,
        users: HashMap::new(),
        people: people(&records),
        summary: Summary::default(),
        files: Vec::new(),
    };
    importer.run(&mut tx).await?;
    // Attachment files are written once the records are in.
    let files = std::mem::take(&mut importer.files);
    let mut summary = importer.summary;
    tx.commit().await?;
    // Jira keeps a file at `data/attachments/<project>/<bucket>/<issue key>/<attachment id>`.
    let mut written = 0;
    let mut missing = 0;
    if backup.is_zip() {
        let dir = data_dir.join("attachments");
        std::fs::create_dir_all(&dir)?;
        for (id, jira_id, key) in files {
            let candidates = backup.entries_named(&jira_id);
            let entry = candidates
                .iter()
                .find(|n| n.contains("attachments") && n.contains(&format!("/{key}/")))
                .or_else(|| candidates.iter().find(|n| n.contains("attachments")))
                .cloned();
            match entry {
                Some(entry) => {
                    let bytes = backup.read(&entry)?;
                    std::fs::write(dir.join(id.to_string()), bytes)?;
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

/// People by the key other records name them with: a user key, or a user name.
fn people(records: &Records) -> HashMap<String, ImportedUser> {
    let mut by_name: HashMap<String, ImportedUser> = HashMap::new();
    for user in records.of("User") {
        let name = user.text("lowerUserName");
        let name = if name.is_empty() {
            user.text("userName").to_lowercase()
        } else {
            name
        };
        by_name.insert(
            name.clone(),
            ImportedUser {
                key: name.clone(),
                name: user.text("userName"),
                display_name: user.text("displayName"),
                email: user.text("emailAddress"),
                active: user.get("active").is_none_or(|a| a == "1" || a == "true"),
            },
        );
    }
    let mut out = by_name.clone();
    for app_user in records.of("ApplicationUser") {
        let key = app_user.text("userKey");
        let name = app_user.text("lowerUserName");
        let mut person = by_name.get(&name).cloned().unwrap_or(ImportedUser {
            key: key.clone(),
            name: name.clone(),
            display_name: name.clone(),
            email: String::new(),
            active: true,
        });
        person.key = key.clone();
        // Records name people by either key; both must describe the same person, so whichever
        // is imported first keeps the account id.
        out.insert(name, person.clone());
        out.insert(key, person);
    }
    out
}

struct Importer<'a> {
    records: &'a Records,
    /// Tessera user ids by backup user key.
    users: HashMap<String, i64>,
    people: HashMap<String, ImportedUser>,
    summary: Summary,
    /// Attachment files to copy: Tessera's attachment id, Jira's, and the issue key.
    files: Vec<(i64, String, String)>,
}

impl Importer<'_> {
    async fn user(&mut self, conn: &mut SqliteConnection, key: &str) -> ImportResult<Option<i64>> {
        let key = key.trim();
        if key.is_empty() {
            return Ok(None);
        }
        if let Some(id) = self.users.get(key) {
            return Ok(Some(*id));
        }
        let person = self
            .people
            .get(key)
            .or_else(|| self.people.get(&key.to_lowercase()))
            .cloned()
            .unwrap_or(ImportedUser {
                key: key.to_string(),
                name: key.to_string(),
                display_name: key.to_string(),
                email: String::new(),
                active: true,
            });
        let id = user_for(conn, &person).await?;
        self.users.insert(key.to_string(), id);
        Ok(Some(id))
    }

    /// The account id of a user, for history values.
    async fn account_of(
        &mut self,
        conn: &mut SqliteConnection,
        key: &str,
    ) -> ImportResult<Option<String>> {
        match self.user(conn, key).await? {
            Some(id) => Ok(
                sqlx::query_scalar("SELECT account_id FROM users WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?,
            ),
            None => Ok(None),
        }
    }

    async fn run(&mut self, conn: &mut SqliteConnection) -> ImportResult<()> {
        let records = self.records;
        // People, and the groups they belong to.
        let people: Vec<String> = self.people.keys().cloned().collect();
        for key in &people {
            self.user(conn, key).await?;
        }
        // One person can be known by both a name and a user key.
        let distinct: std::collections::HashSet<i64> = self.users.values().copied().collect();
        self.summary.count("users", distinct.len());
        let mut groups = HashMap::new();
        for group in records.of("Group") {
            let name = group.text("groupName");
            if !name.is_empty() {
                groups.insert(name.to_lowercase(), group_for(conn, &name).await?);
            }
        }
        for membership in records.of("Membership") {
            if membership
                .get("membershipType")
                .is_some_and(|t| t != "GROUP_USER")
            {
                continue;
            }
            let group_name = membership.text("parentName");
            let group = match groups.get(&group_name.to_lowercase()) {
                Some(id) => *id,
                None if !group_name.is_empty() => group_for(conn, &group_name).await?,
                None => continue,
            };
            if let Some(user) = self.user(conn, &membership.text("lowerChildName")).await? {
                sqlx::query(
                    "INSERT OR IGNORE INTO group_members (group_id, user_id) VALUES (?, ?)",
                )
                .bind(group)
                .bind(user)
                .execute(&mut *conn)
                .await?;
            }
        }
        self.summary.count("groups", groups.len());

        let catalog = self.catalog(conn).await?;
        let fallback = self.fallback_user(conn).await?;

        // Projects, with their components and versions.
        let mut projects: HashMap<String, (i64, String)> = HashMap::new();
        for project in records.of("Project") {
            let key = project.text("key");
            let exists: Option<i64> =
                sqlx::query_scalar("SELECT id FROM projects WHERE key = ? COLLATE NOCASE")
                    .bind(&key)
                    .fetch_optional(&mut *conn)
                    .await?;
            if exists.is_some() || key.is_empty() {
                self.summary.notes.push(format!(
                    "Project {key} was skipped: Tessera already has a project with that key."
                ));
                continue;
            }
            let lead = self.user(conn, &project.text("lead")).await?;
            let assignee_type = if project.get("assigneetype") == Some("2") {
                "PROJECT_LEAD"
            } else {
                "UNASSIGNED"
            };
            let project_type = project
                .get("projecttype")
                .filter(|t| matches!(*t, "software" | "business" | "service_desk"))
                .unwrap_or("software");
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO projects (key, name, description, lead_id, url, assignee_type,
                    project_type_key, workflow_id, issue_counter, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?) RETURNING id",
            )
            .bind(&key)
            .bind(project.text("name"))
            .bind(project.text("description"))
            .bind(lead)
            .bind(project.text("url"))
            .bind(assignee_type)
            .bind(project_type)
            .bind(project.int("pcounter").unwrap_or(0))
            .bind(now_ms())
            .fetch_one(&mut *conn)
            .await?;
            // The lead administers the project, as on a new Tessera project.
            if let Some(lead) = lead.or(Some(fallback)) {
                sqlx::query("INSERT OR IGNORE INTO project_role_users (project_id, role_id, user_id) VALUES (?, 10002, ?)")
                    .bind(id)
                    .bind(lead)
                    .execute(&mut *conn)
                    .await?;
            }
            projects.insert(project.text("id"), (id, key));
        }
        self.summary.count("projects", projects.len());

        let mut components = HashMap::new();
        for component in records.of("Component") {
            let Some((project, _)) = projects.get(&component.text("project")) else {
                continue;
            };
            let lead = self.user(conn, &component.text("lead")).await?;
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO components (project_id, name, description, lead_id) VALUES (?, ?, ?, ?)
                 ON CONFLICT (project_id, name) DO UPDATE SET description = excluded.description
                 RETURNING id",
            )
            .bind(project)
            .bind(component.text("name"))
            .bind(component.text("description"))
            .bind(lead)
            .fetch_one(&mut *conn)
            .await?;
            components.insert(component.text("id"), id);
        }
        let mut versions = HashMap::new();
        for version in records.of("Version") {
            let Some((project, _)) = projects.get(&version.text("project")) else {
                continue;
            };
            let date = |name: &str| {
                version
                    .get(name)
                    .and_then(parse_time)
                    .map(crate::time::format_date)
            };
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO versions (project_id, name, description, archived, released, start_date,
                    release_date, sequence)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT (project_id, name) DO UPDATE SET description = excluded.description
                 RETURNING id",
            )
            .bind(project)
            .bind(version.text("name"))
            .bind(version.text("description"))
            .bind(version.get("archived").is_some_and(|v| v == "true" || v == "1"))
            .bind(version.get("released").is_some_and(|v| v == "true" || v == "1"))
            .bind(date("startdate"))
            .bind(date("releasedate"))
            .bind(version.int("sequence").unwrap_or(0))
            .fetch_one(&mut *conn)
            .await?;
            versions.insert(version.text("id"), id);
        }
        self.summary.count("components", components.len());
        self.summary.count("versions", versions.len());

        // Issues, oldest first, keeping their keys.
        let mut issue_rows: Vec<&Entity> = records
            .of("Issue")
            .iter()
            .filter(|i| projects.contains_key(&i.text("project")))
            .collect();
        issue_rows.sort_by_key(|i| {
            (
                i.get("created").and_then(parse_time).unwrap_or(0),
                i.int("id"),
            )
        });
        let mut issues: HashMap<String, (i64, String)> = HashMap::new();
        let mut last_rank: Option<String> = sqlx::query_scalar("SELECT MAX(lexo_rank) FROM issues")
            .fetch_one(&mut *conn)
            .await?;
        let mut highest: HashMap<i64, i64> = HashMap::new();
        for issue in issue_rows {
            let (project_id, project_key) = projects[&issue.text("project")].clone();
            let Some(number) = issue.int("number").or_else(|| {
                issue
                    .get("pkey")
                    .and_then(|k| k.rsplit('-').next()?.parse().ok())
            }) else {
                continue;
            };
            let key = format!("{project_key}-{number}");
            let (description, description_text) = rich(&issue.text("description"));
            let (environment, environment_text) = rich(&issue.text("environment"));
            let reporter = self.user(conn, &issue.text("reporter")).await?;
            let assignee = self.user(conn, &issue.text("assignee")).await?;
            let creator = self
                .user(conn, &issue.text("creator"))
                .await?
                .or(reporter)
                .unwrap_or(fallback);
            let created = issue
                .get("created")
                .and_then(parse_time)
                .unwrap_or_else(now_ms);
            let rank = ranks::after(last_rank.as_deref());
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO issues (project_id, number, key, issue_type_id, summary, description,
                    environment, description_text, environment_text, status_id, priority_id,
                    resolution_id, resolution_date, assignee_id, reporter_id, creator_id, due_date,
                    created_at, updated_at, original_estimate, remaining_estimate, lexo_rank)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(project_id)
            .bind(number)
            .bind(&key)
            .bind(catalog.issue_type(&issue.text("type")))
            .bind(issue.text("summary"))
            .bind(description)
            .bind(environment)
            .bind(description_text)
            .bind(environment_text)
            .bind(catalog.status(&issue.text("status")))
            .bind(catalog.priority(&issue.text("priority")))
            .bind(catalog.resolution(&issue.text("resolution")))
            .bind(issue.get("resolutiondate").and_then(parse_time))
            .bind(assignee)
            .bind(reporter)
            .bind(creator)
            .bind(issue.get("duedate").and_then(parse_time).map(crate::time::format_date))
            .bind(created)
            .bind(issue.get("updated").and_then(parse_time).unwrap_or(created))
            .bind(issue.int("timeoriginalestimate"))
            .bind(issue.int("timeestimate"))
            .bind(&rank)
            .fetch_one(&mut *conn)
            .await?;
            last_rank = Some(rank);
            let high = highest.entry(project_id).or_default();
            *high = (*high).max(number);
            issues.insert(issue.text("id"), (id, key));
        }
        for (project, high) in &highest {
            sqlx::query("UPDATE projects SET issue_counter = MAX(issue_counter, ?) WHERE id = ?")
                .bind(high)
                .bind(project)
                .execute(&mut *conn)
                .await?;
        }
        self.summary.count("issues", issues.len());
        let issue_id = |jira_id: &str| issues.get(jira_id).map(|(id, _)| *id);

        // Links: subtasks and epics become parents, the rest issue links.
        let link_types: HashMap<String, &Entity> = records
            .of("IssueLinkType")
            .iter()
            .map(|t| (t.text("id"), t))
            .collect();
        let mut link_count = 0;
        for link in records.of("IssueLink") {
            let (Some(source), Some(destination)) = (
                issue_id(&link.text("source")),
                issue_id(&link.text("destination")),
            ) else {
                continue;
            };
            let Some(kind) = link_types.get(&link.text("linktype")) else {
                continue;
            };
            let name = kind.text("linkname");
            if kind.get("style") == Some("jira_subtask")
                || name.eq_ignore_ascii_case("Epic-Story Link")
            {
                sqlx::query("UPDATE issues SET parent_id = ? WHERE id = ?")
                    .bind(source)
                    .bind(destination)
                    .execute(&mut *conn)
                    .await?;
                continue;
            }
            let type_id: i64 = sqlx::query_scalar(
                "INSERT INTO issue_link_types (name, inward, outward) VALUES (?, ?, ?)
                 ON CONFLICT (name) DO UPDATE SET name = excluded.name RETURNING id",
            )
            .bind(&name)
            .bind(kind.text("inward"))
            .bind(kind.text("outward"))
            .fetch_one(&mut *conn)
            .await?;
            sqlx::query(
                "INSERT OR IGNORE INTO issue_links (link_type_id, outward_issue_id, inward_issue_id, created_at)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(type_id)
            .bind(source)
            .bind(destination)
            .bind(now_ms())
            .execute(&mut *conn)
            .await?;
            link_count += 1;
        }
        self.summary.count("issue links", link_count);

        // Comments.
        let mut comment_count = 0;
        for action in records.of("Action") {
            if action.get("type").is_some_and(|t| t != "comment") {
                continue;
            }
            let Some(issue) = issue_id(&action.text("issue")) else {
                continue;
            };
            let author = self
                .user(conn, &action.text("author"))
                .await?
                .unwrap_or(fallback);
            let updater = self
                .user(conn, &action.text("updateauthor"))
                .await?
                .unwrap_or(author);
            let (body, text) = rich(&action.text("body"));
            let created = action
                .get("created")
                .and_then(parse_time)
                .unwrap_or_else(now_ms);
            let body = body.unwrap_or_else(|| {
                serde_json::to_string(&tessera_adf::Node::doc(Vec::new())).unwrap_or_default()
            });
            sqlx::query(
                "INSERT INTO comments (issue_id, author_id, update_author_id, body, body_text, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(issue)
            .bind(author)
            .bind(updater)
            .bind(body)
            .bind(text)
            .bind(created)
            .bind(action.get("updated").and_then(parse_time).unwrap_or(created))
            .execute(&mut *conn)
            .await?;
            comment_count += 1;
        }
        self.summary.count("comments", comment_count);

        // Labels, components, versions, watchers, and votes.
        for label in records.of("Label") {
            if label.get("fieldid").is_some() {
                continue;
            }
            if let Some(issue) = issue_id(&label.text("issue")) {
                sqlx::query("INSERT OR IGNORE INTO issue_labels (issue_id, label) VALUES (?, ?)")
                    .bind(issue)
                    .bind(label.text("label"))
                    .execute(&mut *conn)
                    .await?;
            }
        }
        for association in records.of("NodeAssociation") {
            let Some(issue) = issue_id(
                &association
                    .text("sourceNodeId")
                    .or_text(association, "source_node_id"),
            ) else {
                continue;
            };
            let sink = association
                .text("sinkNodeId")
                .or_text(association, "sink_node_id");
            let kind = association
                .text("associationType")
                .or_text(association, "association_type");
            match kind.as_str() {
                "IssueComponent" => {
                    if let Some(component) = components.get(&sink) {
                        sqlx::query("INSERT OR IGNORE INTO issue_components (issue_id, component_id) VALUES (?, ?)")
                            .bind(issue)
                            .bind(component)
                            .execute(&mut *conn)
                            .await?;
                    }
                }
                "IssueFixVersion" | "IssueVersion" => {
                    if let Some(version) = versions.get(&sink) {
                        let which = if kind == "IssueFixVersion" {
                            "fix"
                        } else {
                            "affects"
                        };
                        sqlx::query("INSERT OR IGNORE INTO issue_versions (issue_id, version_id, kind) VALUES (?, ?, ?)")
                            .bind(issue)
                            .bind(version)
                            .bind(which)
                            .execute(&mut *conn)
                            .await?;
                    }
                }
                _ => {}
            }
        }
        for association in records.of("UserAssociation") {
            let table = match association.get("associationType") {
                Some("WatchIssue") => "watchers",
                Some("VoteIssue") => "votes",
                _ => continue,
            };
            let Some(issue) = issue_id(&association.text("sinkNodeId")) else {
                continue;
            };
            if let Some(user) = self.user(conn, &association.text("sourceName")).await? {
                // Safe: the table name is one of two fixed names above.
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "INSERT OR IGNORE INTO {table} (issue_id, user_id) VALUES (?, ?)"
                )))
                .bind(issue)
                .bind(user)
                .execute(&mut *conn)
                .await?;
            }
        }

        // Worklogs.
        let mut worklog_count = 0;
        for worklog in records.of("Worklog") {
            let Some(issue) = issue_id(&worklog.text("issue")) else {
                continue;
            };
            let author = self
                .user(conn, &worklog.text("author"))
                .await?
                .unwrap_or(fallback);
            let updater = self
                .user(conn, &worklog.text("updateauthor"))
                .await?
                .unwrap_or(author);
            let (comment, comment_text) = rich(&worklog.text("body"));
            let created = worklog
                .get("created")
                .and_then(parse_time)
                .unwrap_or_else(now_ms);
            sqlx::query(
                "INSERT INTO worklogs (issue_id, author_id, update_author_id, comment, comment_text,
                    started, time_spent_seconds, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(issue)
            .bind(author)
            .bind(updater)
            .bind(comment)
            .bind(comment_text)
            .bind(worklog.get("startdate").and_then(parse_time).unwrap_or(created))
            .bind(worklog.int("timeworked").unwrap_or(0))
            .bind(created)
            .bind(worklog.get("updated").and_then(parse_time).unwrap_or(created))
            .execute(&mut *conn)
            .await?;
            worklog_count += 1;
        }
        self.summary.count("worklogs", worklog_count);

        self.history(conn, &issues, &catalog, fallback).await?;
        self.custom_fields(conn, &issues).await?;
        self.attachments(conn, &issues, fallback).await?;
        self.fit_workflows(conn, &projects).await?;
        Ok(())
    }

    /// Someone to stand in for authors a backup does not name: the first administrator.
    async fn fallback_user(&mut self, conn: &mut SqliteConnection) -> ImportResult<i64> {
        let admin: Option<i64> =
            sqlx::query_scalar("SELECT id FROM users WHERE is_admin = 1 ORDER BY id LIMIT 1")
                .fetch_optional(&mut *conn)
                .await?;
        match admin {
            Some(id) => Ok(id),
            None => Ok(self.user(conn, "imported-user").await?.expect("a key")),
        }
    }

    /// Maps the backup's issue types, statuses, priorities, and resolutions to Tessera's by
    /// name, adding the ones Tessera lacks.
    async fn catalog(&mut self, conn: &mut SqliteConnection) -> ImportResult<CatalogMap> {
        let records = self.records;
        let mut map = CatalogMap::default();
        async fn find_or_add(
            conn: &mut SqliteConnection,
            table: &str,
            name: &str,
            insert: &str,
            binds: &[Value],
        ) -> ImportResult<i64> {
            // Safe: table names and statements are fixed by the callers below.
            let found: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT id FROM {table} WHERE name = ? COLLATE NOCASE"
            )))
            .bind(name)
            .fetch_optional(&mut *conn)
            .await?;
            if let Some(id) = found {
                return Ok(id);
            }
            let mut query = sqlx::query_scalar(sqlx::AssertSqlSafe(insert.to_string())).bind(name);
            for bind in binds {
                query = match bind {
                    Value::Number(n) => query.bind(n.as_i64()),
                    other => query.bind(other.as_str().unwrap_or_default().to_string()),
                };
            }
            Ok(query.fetch_one(&mut *conn).await?)
        }
        for kind in records.of("IssueType") {
            let name = kind.text("name");
            let level = if kind.get("style") == Some("jira_subtask") {
                -1
            } else if name.eq_ignore_ascii_case("Epic") {
                1
            } else {
                0
            };
            let id = find_or_add(
                conn,
                "issue_types",
                &name,
                "INSERT INTO issue_types (name, description, hierarchy_level) VALUES (?, ?, ?) RETURNING id",
                &[json!(kind.text("description")), json!(level)],
            )
            .await?;
            map.issue_types.insert(kind.text("id"), id);
        }
        for status in records.of("Status") {
            let category = status
                .int("statuscategory")
                .filter(|c| (2..=4).contains(c))
                .unwrap_or(2);
            let id = find_or_add(
                conn,
                "statuses",
                &status.text("name"),
                "INSERT INTO statuses (name, description, category_id) VALUES (?, ?, ?) RETURNING id",
                &[json!(status.text("description")), json!(category)],
            )
            .await?;
            map.statuses.insert(status.text("id"), id);
        }
        for priority in records.of("Priority") {
            let next: i64 =
                sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) + 1 FROM priorities")
                    .fetch_one(&mut *conn)
                    .await?;
            let color = priority.get("statusColor").unwrap_or("#707070").to_string();
            let id = find_or_add(
                conn,
                "priorities",
                &priority.text("name"),
                "INSERT INTO priorities (name, description, status_color, sequence) VALUES (?, ?, ?, ?) RETURNING id",
                &[json!(priority.text("description")), json!(color), json!(next)],
            )
            .await?;
            map.priorities.insert(priority.text("id"), id);
        }
        for resolution in records.of("Resolution") {
            let id = find_or_add(
                conn,
                "resolutions",
                &resolution.text("name"),
                "INSERT INTO resolutions (name, description) VALUES (?, ?) RETURNING id",
                &[json!(resolution.text("description"))],
            )
            .await?;
            map.resolutions.insert(resolution.text("id"), id);
        }
        map.default_type = sqlx::query_scalar("SELECT id FROM issue_types WHERE name = 'Task'")
            .fetch_optional(&mut *conn)
            .await?
            .unwrap_or(10002);
        map.default_status = sqlx::query_scalar("SELECT id FROM statuses ORDER BY id LIMIT 1")
            .fetch_one(&mut *conn)
            .await?;
        Ok(map)
    }

    /// Issue history, with ids in status, priority, and similar fields turned into Tessera's.
    async fn history(
        &mut self,
        conn: &mut SqliteConnection,
        issues: &HashMap<String, (i64, String)>,
        catalog: &CatalogMap,
        fallback: i64,
    ) -> ImportResult<()> {
        let records = self.records;
        let mut items: HashMap<String, Vec<&Entity>> = HashMap::new();
        for item in records.of("ChangeItem") {
            items.entry(item.text("group")).or_default().push(item);
        }
        let mut count = 0;
        for group in records.of("ChangeGroup") {
            let Some((issue, _)) = issues.get(&group.text("issue")) else {
                continue;
            };
            let Some(changes) = items.get(&group.text("id")) else {
                continue;
            };
            let author = self
                .user(conn, &group.text("author"))
                .await?
                .unwrap_or(fallback);
            let created = group
                .get("created")
                .and_then(parse_time)
                .unwrap_or_else(now_ms);
            let changelog: i64 = sqlx::query_scalar(
                "INSERT INTO changelogs (issue_id, author_id, created_at) VALUES (?, ?, ?) RETURNING id",
            )
            .bind(issue)
            .bind(author)
            .bind(created)
            .fetch_one(&mut *conn)
            .await?;
            for item in changes {
                let field = item.text("field");
                let mut from = item.get("oldvalue").map(str::to_string);
                let mut to = item.get("newvalue").map(str::to_string);
                let lookup: Option<&HashMap<String, i64>> = match field.as_str() {
                    "status" => Some(&catalog.statuses),
                    "priority" => Some(&catalog.priorities),
                    "resolution" => Some(&catalog.resolutions),
                    "issuetype" => Some(&catalog.issue_types),
                    _ => None,
                };
                if let Some(lookup) = lookup {
                    from = from.and_then(|v| lookup.get(&v).map(i64::to_string));
                    to = to.and_then(|v| lookup.get(&v).map(i64::to_string));
                } else if matches!(field.as_str(), "assignee" | "reporter") {
                    from = match from {
                        Some(key) => self.account_of(conn, &key).await?,
                        None => None,
                    };
                    to = match to {
                        Some(key) => self.account_of(conn, &key).await?,
                        None => None,
                    };
                }
                sqlx::query(
                    "INSERT INTO changelog_items (changelog_id, field, field_id, field_type, from_value,
                        from_string, to_value, to_string)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(changelog)
                .bind(&field)
                .bind(&field)
                .bind(item.get("fieldtype").unwrap_or("jira"))
                .bind(from)
                .bind(item.get("oldstring"))
                .bind(to)
                .bind(item.get("newstring"))
                .execute(&mut *conn)
                .await?;
            }
            count += 1;
        }
        self.summary.count("history entries", count);
        Ok(())
    }

    /// Custom fields of the kinds Tessera has, and their values.
    async fn custom_fields(
        &mut self,
        conn: &mut SqliteConnection,
        issues: &HashMap<String, (i64, String)>,
    ) -> ImportResult<()> {
        let records = self.records;
        // Jira field type key to Tessera field type.
        let kind_of = |key: &str| -> Option<&'static str> {
            let last = key.rsplit(':').next().unwrap_or(key);
            Some(match last {
                "textfield" | "readonlyfield" => "string",
                "textarea" => "text",
                "float" | "jsw-story-points" => "number",
                "datepicker" => "date",
                "datetime" => "datetime",
                "url" => "url",
                "select" | "radiobuttons" => "option",
                "multiselect" | "multicheckboxes" => "options",
                "labels" => "labels",
                "userpicker" => "user",
                _ => return None,
            })
        };
        let mut fields: HashMap<String, (i64, &'static str)> = HashMap::new();
        let mut skipped = HashSet::new();
        for field in records.of("CustomField") {
            let name = field.text("name");
            let Some(kind) = kind_of(&field.text("customfieldtypekey")) else {
                skipped.insert(name);
                continue;
            };
            // Story points go into Tessera's own estimate field.
            let name = if name.eq_ignore_ascii_case("Story Points") {
                "Story point estimate".to_string()
            } else {
                name
            };
            let existing: Option<(i64, String)> = sqlx::query_as(
                "SELECT id, field_type FROM custom_fields WHERE name = ? COLLATE NOCASE",
            )
            .bind(&name)
            .fetch_optional(&mut *conn)
            .await?;
            let id = match existing {
                Some((id, _)) => id,
                None => sqlx::query_scalar(
                    "INSERT INTO custom_fields (name, description, field_type) VALUES (?, ?, ?) RETURNING id",
                )
                .bind(&name)
                .bind(field.text("description"))
                .bind(kind)
                .fetch_one(&mut *conn)
                .await?,
            };
            fields.insert(field.text("id"), (id, kind));
        }
        let mut options = HashMap::new();
        for option in records.of("CustomFieldOption") {
            let Some((field, _)) = fields.get(&option.text("customfield")) else {
                continue;
            };
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO custom_field_options (field_id, value, position, disabled) VALUES (?, ?, ?, ?) RETURNING id",
            )
            .bind(field)
            .bind(option.text("value"))
            .bind(option.int("sequence").unwrap_or(0))
            .bind(option.get("disabled").is_some_and(|d| d == "Y" || d == "true"))
            .fetch_one(&mut *conn)
            .await?;
            options.insert(option.text("id"), id);
        }
        // Values for one issue and field, gathered first because multi-valued fields repeat.
        let mut gathered: HashMap<(i64, i64), (&'static str, Vec<&Entity>)> = HashMap::new();
        for value in records.of("CustomFieldValue") {
            let (Some((issue, _)), Some((field, kind))) = (
                issues.get(&value.text("issue")),
                fields.get(&value.text("customfield")),
            ) else {
                continue;
            };
            gathered
                .entry((*issue, *field))
                .or_insert((kind, Vec::new()))
                .1
                .push(value);
        }
        let mut count = 0;
        for ((issue, field), (kind, values)) in gathered {
            let first = values[0];
            let stored = match kind {
                "string" | "url" => first.get("stringvalue").map(|s| json!(s)),
                "text" => first
                    .get("textvalue")
                    .or_else(|| first.get("stringvalue"))
                    .map(|t| {
                        serde_json::to_value(tessera_adf::wiki::from_wiki_markup(t))
                            .unwrap_or(Value::Null)
                    }),
                "number" => first
                    .get("numbervalue")
                    .and_then(|n| n.parse::<f64>().ok())
                    .map(|n| json!(n)),
                "date" => first
                    .get("datevalue")
                    .and_then(parse_time)
                    .map(|ms| json!(crate::time::format_date(ms))),
                "datetime" => first
                    .get("datevalue")
                    .and_then(parse_time)
                    .map(|ms| json!(crate::time::format(ms))),
                "option" => first
                    .get("stringvalue")
                    .and_then(|o| options.get(o))
                    .map(|id| json!({ "id": id.to_string() })),
                "options" => {
                    let ids: Vec<Value> = values
                        .iter()
                        .filter_map(|v| v.get("stringvalue").and_then(|o| options.get(o)))
                        .map(|id| json!({ "id": id.to_string() }))
                        .collect();
                    (!ids.is_empty()).then(|| json!(ids))
                }
                "labels" => {
                    let labels: Vec<&str> =
                        values.iter().filter_map(|v| v.get("stringvalue")).collect();
                    (!labels.is_empty()).then(|| json!(labels))
                }
                "user" => match first.get("stringvalue") {
                    Some(key) => self
                        .account_of(conn, key)
                        .await?
                        .map(|a| json!({ "accountId": a })),
                    None => None,
                },
                _ => None,
            };
            let Some(stored) = stored else {
                continue;
            };
            sqlx::query("INSERT OR REPLACE INTO issue_field_values (issue_id, field_id, value) VALUES (?, ?, ?)")
                .bind(issue)
                .bind(field)
                .bind(stored.to_string())
                .execute(&mut *conn)
                .await?;
            count += 1;
        }
        self.summary.count("custom field values", count);
        if !skipped.is_empty() {
            let mut names: Vec<String> = skipped.into_iter().collect();
            names.sort();
            self.summary.notes.push(format!(
                "Custom fields of kinds Tessera lacks were skipped: {}.",
                names.join(", ")
            ));
        }
        Ok(())
    }

    /// Attachment records, with their files found in the zip.
    async fn attachments(
        &mut self,
        conn: &mut SqliteConnection,
        issues: &HashMap<String, (i64, String)>,
        fallback: i64,
    ) -> ImportResult<()> {
        let records = self.records;
        let mut count = 0;
        for attachment in records.of("FileAttachment") {
            let Some((issue, key)) = issues.get(&attachment.text("issue")) else {
                continue;
            };
            let author = self
                .user(conn, &attachment.text("author"))
                .await?
                .unwrap_or(fallback);
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO attachments (issue_id, author_id, filename, mime_type, size, created_at)
                 VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(issue)
            .bind(author)
            .bind(attachment.text("filename"))
            .bind(attachment.get("mimetype").unwrap_or("application/octet-stream"))
            .bind(attachment.int("filesize").unwrap_or(0))
            .bind(attachment.get("created").and_then(parse_time).unwrap_or_else(now_ms))
            .fetch_one(&mut *conn)
            .await?;
            self.files.push((id, attachment.text("id"), key.clone()));
            count += 1;
        }
        self.summary.count("attachments", count);
        Ok(())
    }

    /// Gives each project a workflow with every status its issues use, so imported statuses
    /// stay as they were. Any status can move to any other, as in Jira's simplified workflow.
    async fn fit_workflows(
        &mut self,
        conn: &mut SqliteConnection,
        projects: &HashMap<String, (i64, String)>,
    ) -> ImportResult<()> {
        let default: Vec<i64> =
            sqlx::query_scalar("SELECT status_id FROM workflow_statuses WHERE workflow_id = 1")
                .fetch_all(&mut *conn)
                .await?;
        for (project, key) in projects.values() {
            let used: Vec<i64> =
                sqlx::query_scalar("SELECT DISTINCT status_id FROM issues WHERE project_id = ?")
                    .bind(project)
                    .fetch_all(&mut *conn)
                    .await?;
            if used.iter().all(|s| default.contains(s)) {
                continue;
            }
            let mut statuses = default.clone();
            for status in used {
                if !statuses.contains(&status) {
                    statuses.push(status);
                }
            }
            let workflow: i64 = sqlx::query_scalar(
                "INSERT INTO workflows (name, description) VALUES (?, ?) RETURNING id",
            )
            .bind(format!("{key} workflow"))
            .bind("Imported from Jira: every status the project's issues use.")
            .fetch_one(&mut *conn)
            .await?;
            let initial = statuses[0];
            let mut next: i64 =
                sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM workflow_transitions")
                    .fetch_one(&mut *conn)
                    .await?;
            for status in &statuses {
                sqlx::query("INSERT INTO workflow_statuses (workflow_id, status_id, is_initial) VALUES (?, ?, ?)")
                    .bind(workflow)
                    .bind(status)
                    .bind(*status == initial)
                    .execute(&mut *conn)
                    .await?;
                let name: String = sqlx::query_scalar("SELECT name FROM statuses WHERE id = ?")
                    .bind(status)
                    .fetch_one(&mut *conn)
                    .await?;
                next = (next / 10 + 1) * 10 + 1;
                sqlx::query(
                    "INSERT INTO workflow_transitions (id, workflow_id, name, from_status_id, to_status_id)
                     VALUES (?, ?, ?, NULL, ?)",
                )
                .bind(next)
                .bind(workflow)
                .bind(name)
                .bind(status)
                .execute(&mut *conn)
                .await?;
            }
            let scheme: i64 = sqlx::query_scalar(
                "INSERT INTO workflow_schemes (name, description, default_workflow_id) VALUES (?, ?, ?) RETURNING id",
            )
            .bind(format!("{key} workflow scheme"))
            .bind("Imported from Jira.")
            .bind(workflow)
            .fetch_one(&mut *conn)
            .await?;
            sqlx::query("UPDATE projects SET workflow_id = ?, workflow_scheme_id = ? WHERE id = ?")
                .bind(workflow)
                .bind(scheme)
                .bind(project)
                .execute(&mut *conn)
                .await?;
        }
        Ok(())
    }
}

/// The backup's metadata ids mapped to Tessera's.
#[derive(Default)]
struct CatalogMap {
    issue_types: HashMap<String, i64>,
    statuses: HashMap<String, i64>,
    priorities: HashMap<String, i64>,
    resolutions: HashMap<String, i64>,
    default_type: i64,
    default_status: i64,
}

impl CatalogMap {
    fn issue_type(&self, id: &str) -> i64 {
        self.issue_types
            .get(id)
            .copied()
            .unwrap_or(self.default_type)
    }

    fn status(&self, id: &str) -> i64 {
        self.statuses
            .get(id)
            .copied()
            .unwrap_or(self.default_status)
    }

    fn priority(&self, id: &str) -> Option<i64> {
        self.priorities.get(id).copied()
    }

    fn resolution(&self, id: &str) -> Option<i64> {
        self.resolutions.get(id).copied()
    }
}

/// A field that may go by another name in some backups.
trait OrText {
    fn or_text(self, entity: &Entity, other: &str) -> String;
}

impl OrText for String {
    fn or_text(self, entity: &Entity, other: &str) -> String {
        if self.is_empty() {
            entity.text(other)
        } else {
            self
        }
    }
}
