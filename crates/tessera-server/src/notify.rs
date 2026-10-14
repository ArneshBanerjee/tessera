//! Email notifications for issue events, following Jira's default notification scheme: the
//! assignee, the reporter, and the watchers hear about each change, but never about their own,
//! and only about issues they may see.

use std::collections::BTreeSet;

use crate::AppState;
use crate::api::access::Access;
use crate::store::issues::Issue;
use crate::store::users::{self, User};
use crate::store::{outbox, projects, social};

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
