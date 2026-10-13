//! `/`: the user's open work and recently updated issues.

use leptos::prelude::*;

use super::search::IssueTable;
use crate::app::use_session;

#[component]
fn IssueList(title: &'static str, jql: &'static str) -> impl IntoView {
    let session = use_session();
    let issues = LocalResource::new(move || {
        session.changes.track();
        async move {
            super::search(jql, 0, 20)
                .await
                .map(|page| page.issues)
                .unwrap_or_default()
        }
    });
    view! {
        <section class="work-section">
            <h2>{title}</h2>
            <Transition fallback=|| view! { <p class="muted">"Loading…"</p> }>
                {move || issues.get().map(|list| view! { <IssueTable issues=list/> })}
            </Transition>
        </section>
    }
}

#[component]
pub fn YourWork() -> impl IntoView {
    let session = use_session();
    let name = session.me.get_value().display_name;
    view! {
        <div class="page">
            <header class="page-header"><h1>{format!("Your work, {name}")}</h1></header>
            <IssueList title="Assigned to you" jql="assignee = currentUser() AND resolution = Unresolved ORDER BY priority DESC, updated DESC"/>
            <IssueList title="Recently updated" jql="updated >= -14d ORDER BY updated DESC"/>
        </div>
    }
}
