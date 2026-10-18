//! Email notifications.
//!
//! - Jira: following the default notification scheme, the assignee, the reporter, and the
//!   watchers hear about each change to an issue.
//! - Confluence: the watchers of a page and of its space hear about new pages, edits, and
//!   comments.
//!
//! Nobody hears about their own changes, or about things they may not see.

use std::collections::BTreeSet;

use crate::AppState;
use crate::api::access::Access;
use crate::confluence::access::{SpaceAccess, can_view};
use crate::store::content::Content;
use crate::store::issues::Issue;
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

/// Queues mail about an issue event. Mail problems never fail the request that caused them.
pub async fn issue(state: &AppState, actor: &User, issue: &Issue, event: IssueEvent<'_>) {
    if !state.mail_enabled {
        return;
    }
    if let Err(error) = queue(state, actor, issue, &event).await {
        tracing::error!(%error, key = %issue.key, "could not queue notifications");
    }
}

async fn queue(
    state: &AppState,
    actor: &User,
    issue: &Issue,
    event: &IssueEvent<'_>,
) -> sqlx::Result<()> {
    let mut ids: BTreeSet<i64> = BTreeSet::new();
    ids.extend(issue.assignee_id);
    ids.extend(issue.reporter_id);
    ids.extend(social::members(&state.db, social::List::Watchers, issue.id).await?);
    ids.remove(&actor.id);
    let Some(project) = projects::by_id(&state.db, issue.project_id).await? else {
        return Ok(());
    };
    let subject = format!("[Tessera] ({}) {}", issue.key, issue.summary);
    let details = event.details();
    let mut body = format!(
        "{} {} {}: {}\n\n",
        actor.display_name,
        event.verb(),
        issue.key,
        issue.summary
    );
    if !details.is_empty() {
        body.push_str(&details);
        body.push_str("\n\n");
    }
    body.push_str(&format!("{}/browse/{}\n\n", state.base_url, issue.key));
    body.push_str("You get this email because you are the assignee or reporter of this issue, or you watch it.\n");
    for id in ids {
        let Some(user) = users::by_id(&state.db, id).await? else {
            continue;
        };
        if !user.active {
            continue;
        }
        let can_see = match Access::load(&state.db, &user, &project).await {
            Ok(access) => access.can_on("BROWSE_PROJECTS", Some(issue)),
            Err(_) => false,
        };
        if can_see {
            outbox::enqueue(&state.db, &user.email, &subject, &body).await?;
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

/// Queues mail for the watchers of a page (or blog post) and of its space.
pub async fn wiki(state: &AppState, actor: &User, page: &Content, event: WikiEvent<'_>) {
    if !state.mail_enabled {
        return;
    }
    if let Err(error) = queue_wiki(state, actor, page, &event).await {
        tracing::error!(%error, id = page.id, "could not queue notifications");
    }
}

async fn queue_wiki(
    state: &AppState,
    actor: &User,
    page: &Content,
    event: &WikiEvent<'_>,
) -> sqlx::Result<()> {
    let mut ids: BTreeSet<i64> = BTreeSet::new();
    ids.extend(wiki_watchers::watchers(&state.db, Target::Content(page.id)).await?);
    ids.extend(wiki_watchers::watchers(&state.db, Target::Space(page.space_id)).await?);
    ids.remove(&actor.id);
    let Some(space) = spaces::by_id(&state.db, page.space_id).await? else {
        return Ok(());
    };
    let (verb, details) = match event {
        WikiEvent::Created => ("created", String::new()),
        WikiEvent::Updated => ("edited", String::new()),
        WikiEvent::Commented(text) => ("commented on", text.to_string()),
    };
    let subject = format!("[Tessera] {} > {}", space.name, page.title);
    let mut body = format!("{} {} {}\n\n", actor.display_name, verb, page.title);
    if !details.is_empty() {
        body.push_str(&details);
        body.push_str("\n\n");
    }
    body.push_str(&format!(
        "{}/wiki/spaces/{}/pages/{}\n\n",
        state.base_url, space.key, page.id
    ));
    body.push_str("You get this email because you watch this page or its space.\n");
    for id in ids {
        let Some(user) = users::by_id(&state.db, id).await? else {
            continue;
        };
        if !user.active {
            continue;
        }
        if SpaceAccess::load(&state.db, &user, space.id)
            .await?
            .can("read", "space")
            && can_view(&state.db, &user, page).await?
        {
            outbox::enqueue(&state.db, &user.email, &subject, &body).await?;
        }
    }
    Ok(())
}
