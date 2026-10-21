//! Notifications, in the app and by email.
//!
//! - Jira: following the default notification scheme, the assignee, the reporter, and the
//!   watchers hear about each change to an issue.
//! - Confluence: the watchers of a page and of its space hear about new pages, edits, and
//!   comments.
//! - Mentions: people mentioned in a description, comment, or page hear about it, even when
//!   they do not watch it.
//!
//! Every notification shows in the app. Email goes out too when an SMTP server is set. Nobody
//! hears about their own changes, or about things they may not see.

use std::collections::BTreeSet;

use crate::AppState;
use crate::api::access::Access;
use crate::confluence::access::{SpaceAccess, can_view};
use crate::store::content::Content;
use crate::store::issues::Issue;
use crate::store::notifications::{self, NewNotification};
use crate::store::users::{self, User};
use crate::store::wiki_watchers::{self, Target};
use crate::store::{outbox, projects, social, spaces};

/// What happened to an issue.
pub enum IssueEvent<'a> {
    Created,
    /// The names of the fields that changed.
    Updated(&'a [String]),
    Assigned,
    Commented(&'a str),
    /// The new status.
    Transitioned(&'a str),
    Deleted,
}

impl IssueEvent<'_> {
    fn verb(&self) -> &'static str {
        match self {
            IssueEvent::Created => "created",
            IssueEvent::Updated(_) => "updated",
            IssueEvent::Assigned => "assigned",
            IssueEvent::Commented(_) => "commented on",
            IssueEvent::Transitioned(_) => "changed the status of",
            IssueEvent::Deleted => "deleted",
        }
    }

    fn details(&self) -> String {
        match self {
            IssueEvent::Updated(fields) if !fields.is_empty() => {
                format!("Changed: {}", fields.join(", "))
            }
            IssueEvent::Commented(text) => text.to_string(),
            IssueEvent::Transitioned(status) => format!("New status: {status}"),
            _ => String::new(),
        }
    }
}

/// One notification, ready to deliver to each recipient.
struct Note<'a> {
    actor: &'a User,
    kind: &'a str,
    /// One line for the app, such as "Ana commented on PROJ-1: Fix login".
    title: String,
    details: String,
    /// The web app path the notification opens.
    link: String,
    subject: String,
    /// Why the recipient gets the email, as its last line.
    reason: &'a str,
}

/// Records a notification for one person, and emails it when mail is set up.
async fn deliver(state: &AppState, recipient: &User, note: &Note<'_>) -> sqlx::Result<()> {
    notifications::create(
        &state.db,
        NewNotification {
            user_id: recipient.id,
            actor_id: Some(note.actor.id),
            kind: note.kind,
            title: &note.title,
            body: &note.details,
            link: &note.link,
        },
    )
    .await?;
    if state.mail_enabled {
        let mut body = format!("{}\n\n", note.title);
        if !note.details.is_empty() {
            body.push_str(&note.details);
            body.push_str("\n\n");
        }
        body.push_str(&format!("{}{}\n\n", state.base_url, note.link));
        body.push_str(note.reason);
        body.push('\n');
        outbox::enqueue(&state.db, &recipient.email, &note.subject, &body).await?;
    }
    Ok(())
}

/// Active users by account id, skipping the actor.
async fn mentioned_users(
    state: &AppState,
    actor: &User,
    account_ids: &[String],
) -> sqlx::Result<Vec<User>> {
    let mut out = Vec::new();
    for account in account_ids {
        if let Some(user) = users::by_account_id(&state.db, account).await?
            && user.active
            && user.id != actor.id
        {
            out.push(user);
        }
    }
    Ok(out)
}

/// Notifies people about an issue event. Notification problems never fail the request that
/// caused them.
pub async fn issue(state: &AppState, actor: &User, issue: &Issue, event: IssueEvent<'_>) {
    issue_with_mentions(state, actor, issue, event, &[]).await;
}

/// Notifies people about an issue event, and the people newly mentioned by it (account ids).
/// Someone mentioned hears about the mention instead of the event.
pub async fn issue_with_mentions(
    state: &AppState,
    actor: &User,
    issue: &Issue,
    event: IssueEvent<'_>,
    mentioned: &[String],
) {
    if let Err(error) = send_issue(state, actor, issue, &event, mentioned, true).await {
        tracing::error!(%error, key = %issue.key, "could not send notifications");
    }
}

/// Notifies only the people newly mentioned (account ids) by an edit, with the edited text.
pub async fn issue_mentions(
    state: &AppState,
    actor: &User,
    issue: &Issue,
    text: &str,
    mentioned: &[String],
) {
    if mentioned.is_empty() {
        return;
    }
    let event = IssueEvent::Commented(text);
    if let Err(error) = send_issue(state, actor, issue, &event, mentioned, false).await {
        tracing::error!(%error, key = %issue.key, "could not send notifications");
    }
}

async fn send_issue(
    state: &AppState,
    actor: &User,
    issue: &Issue,
    event: &IssueEvent<'_>,
    mentioned: &[String],
    everyone: bool,
) -> sqlx::Result<()> {
    let Some(project) = projects::by_id(&state.db, issue.project_id).await? else {
        return Ok(());
    };
    let can_see = async |user: &User| match Access::load(&state.db, user, &project).await {
        Ok(access) => access.can_see(issue),
        Err(_) => false,
    };
    let link = format!("/browse/{}", issue.key);
    let mut told: BTreeSet<i64> = BTreeSet::from([actor.id]);

    if !matches!(event, IssueEvent::Deleted) {
        let note = Note {
            actor,
            kind: "mention",
            title: format!(
                "{} mentioned you on {}: {}",
                actor.display_name, issue.key, issue.summary
            ),
            details: event.details(),
            link: link.clone(),
            subject: format!(
                "[Tessera] {} mentioned you on {}",
                actor.display_name, issue.key
            ),
            reason: "You get this email because someone mentioned you.",
        };
        for user in mentioned_users(state, actor, mentioned).await? {
            if told.insert(user.id) && can_see(&user).await {
                deliver(state, &user, &note).await?;
            }
        }
    }

    if !everyone {
        return Ok(());
    }
    let mut ids: BTreeSet<i64> = BTreeSet::new();
    ids.extend(issue.assignee_id);
    ids.extend(issue.reporter_id);
    ids.extend(social::members(&state.db, social::List::Watchers, issue.id).await?);
    let note = Note {
        actor,
        kind: "issue",
        title: format!(
            "{} {} {}: {}",
            actor.display_name,
            event.verb(),
            issue.key,
            issue.summary
        ),
        details: event.details(),
        link,
        subject: format!("[Tessera] ({}) {}", issue.key, issue.summary),
        reason: "You get this email because you are the assignee or reporter of this issue, or you watch it.",
    };
    for id in ids {
        if !told.insert(id) {
            continue;
        }
        let Some(user) = users::by_id(&state.db, id).await? else {
            continue;
        };
        if user.active && can_see(&user).await {
            deliver(state, &user, &note).await?;
        }
    }
    Ok(())
}

/// What happened to a Confluence page or blog post.
pub enum WikiEvent<'a> {
    Created,
    Updated,
    /// The text of the new comment.
    Commented(&'a str),
}

/// Notifies the watchers of a page (or blog post) and of its space.
pub async fn wiki(state: &AppState, actor: &User, page: &Content, event: WikiEvent<'_>) {
    wiki_with_mentions(state, actor, page, event, &[]).await;
}

/// Notifies watchers, and the people newly mentioned (account ids) in the page or comment.
pub async fn wiki_with_mentions(
    state: &AppState,
    actor: &User,
    page: &Content,
    event: WikiEvent<'_>,
    mentioned: &[String],
) {
    if let Err(error) = send_wiki(state, actor, page, &event, mentioned, true).await {
        tracing::error!(%error, id = page.id, "could not send notifications");
    }
}

/// Notifies only the people newly mentioned (account ids) by an edit of a page or comment.
pub async fn wiki_mentions(state: &AppState, actor: &User, page: &Content, mentioned: &[String]) {
    if mentioned.is_empty() {
        return;
    }
    if let Err(error) = send_wiki(state, actor, page, &WikiEvent::Updated, mentioned, false).await {
        tracing::error!(%error, id = page.id, "could not send notifications");
    }
}

/// The web app path of a page or blog post.
pub fn content_link(space_key: &str, page: &Content) -> String {
    let section = if page.content_type == "blogpost" {
        "blog"
    } else {
        "pages"
    };
    format!("/wiki/spaces/{space_key}/{section}/{}", page.id)
}

async fn send_wiki(
    state: &AppState,
    actor: &User,
    page: &Content,
    event: &WikiEvent<'_>,
    mentioned: &[String],
    everyone: bool,
) -> sqlx::Result<()> {
    let Some(space) = spaces::by_id(&state.db, page.space_id).await? else {
        return Ok(());
    };
    let can_see = async |user: &User| -> sqlx::Result<bool> {
        Ok(SpaceAccess::load(&state.db, user, space.id)
            .await?
            .can("read", "space")
            && can_view(&state.db, user, page).await?)
    };
    let (verb, details) = match event {
        WikiEvent::Created => ("created", String::new()),
        WikiEvent::Updated => ("edited", String::new()),
        WikiEvent::Commented(text) => ("commented on", text.to_string()),
    };
    let link = content_link(&space.key, page);
    let mut told: BTreeSet<i64> = BTreeSet::from([actor.id]);

    let note = Note {
        actor,
        kind: "mention",
        title: format!("{} mentioned you on {}", actor.display_name, page.title),
        details: details.clone(),
        link: link.clone(),
        subject: format!(
            "[Tessera] {} mentioned you on {}",
            actor.display_name, page.title
        ),
        reason: "You get this email because someone mentioned you.",
    };
    for user in mentioned_users(state, actor, mentioned).await? {
        if told.insert(user.id) && can_see(&user).await? {
            deliver(state, &user, &note).await?;
        }
    }

    if !everyone {
        return Ok(());
    }
    let mut ids: BTreeSet<i64> = BTreeSet::new();
    ids.extend(wiki_watchers::watchers(&state.db, Target::Content(page.id)).await?);
    ids.extend(wiki_watchers::watchers(&state.db, Target::Space(page.space_id)).await?);
    let note = Note {
        actor,
        kind: "page",
        title: format!("{} {} {}", actor.display_name, verb, page.title),
        details,
        link,
        subject: format!("[Tessera] {} > {}", space.name, page.title),
        reason: "You get this email because you watch this page or its space.",
    };
    for id in ids {
        if !told.insert(id) {
            continue;
        }
        let Some(user) = users::by_id(&state.db, id).await? else {
            continue;
        };
        if user.active && can_see(&user).await? {
            deliver(state, &user, &note).await?;
        }
    }
    Ok(())
}

/// Account ids an ADF JSON document mentions, that `before` did not already mention.
pub fn new_mentions(after: Option<&str>, before: Option<&str>) -> Vec<String> {
    let parse = |json: Option<&str>| {
        json.and_then(|j| serde_json::from_str::<tessera_adf::Node>(j).ok())
            .map(|doc| tessera_adf::mentions(&doc))
            .unwrap_or_default()
    };
    let old = parse(before);
    parse(after)
        .into_iter()
        .filter(|id| !old.contains(id))
        .collect()
}

/// Account ids a storage-format body mentions, that `before` did not already mention.
pub fn new_storage_mentions(after: &str, before: &str) -> Vec<String> {
    let old = tessera_adf::mentions(&tessera_adf::html::from_storage(before));
    tessera_adf::mentions(&tessera_adf::html::from_storage(after))
        .into_iter()
        .filter(|id| !old.contains(id))
        .collect()
}
