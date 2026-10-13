//! Issue search: the JQL search page, a project's issue list, and the issue table they share.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_query_map};
use serde_json::json;

use super::search as run_search;
use crate::api;
use crate::app::use_session;
use crate::components::{Avatar, ErrorText, Lozenge, TypeIcon};
use crate::model::{Filter, Issue, SearchPage, short_time};
use crate::pages::projects::use_project;

/// A table of issues with the usual columns.
#[component]
pub fn IssueTable(issues: Vec<Issue>) -> impl IntoView {
    if issues.is_empty() {
        return view! { <p class="empty">"No issues match."</p> }.into_any();
    }
    view! {
        <table class="issue-table">
            <thead>
                <tr>
                    <th>"Type"</th><th>"Key"</th><th>"Summary"</th><th>"Status"</th>
                    <th>"Assignee"</th><th>"Priority"</th><th>"Updated"</th>
                </tr>
            </thead>
            <tbody>
                {issues.into_iter().map(|issue| {
                    let fields = issue.fields;
                    let href = format!("/browse/{}", issue.key);
                    let assignee = fields.assignee.clone();
                    let assignee_name = assignee.as_ref().map(|u| u.display_name.clone()).unwrap_or_else(|| "Unassigned".into());
                    view! {
                        <tr>
                            <td><TypeIcon item=fields.issuetype/></td>
                            <td><A href=href.clone() attr:class="issue-key">{issue.key}</A></td>
                            <td><A href=href attr:class="summary-link">{fields.summary}</A></td>
                            <td><Lozenge status=fields.status/></td>
                            <td><span class="person"><Avatar user=assignee/>{assignee_name}</span></td>
                            <td><TypeIcon item=fields.priority/></td>
                            <td class="muted">{short_time(&fields.updated)}</td>
                        </tr>
                    }
                }).collect_view()}
            </tbody>
        </table>
    }
    .into_any()
}

/// Results of a JQL query, with paging.
#[component]
fn Results(jql: Signal<String>) -> impl IntoView {
    let session = use_session();
    let start = RwSignal::new(0i64);
    const PAGE: i64 = 50;
    let results = LocalResource::new(move || {
        let jql = jql.get();
        let start_at = start.get();
        session.changes.track();
        async move { run_search(&jql, start_at, PAGE).await }
    });
    view! {
        <Transition fallback=|| view! { <p class="muted">"Searching…"</p> }>
            {move || results.get().map(|result| match result {
                Ok(SearchPage { issues, total, start_at }) => {
                    let shown_to = (start_at + issues.len() as i64).min(total);
                    let has_prev = start_at > 0;
                    let has_next = start_at + PAGE < total;
                    view! {
                        <div class="result-summary muted">
                            {if total == 0 { "No issues".to_string() } else { format!("{} to {} of {} issues", start_at + 1, shown_to, total) }}
                        </div>
                        <IssueTable issues=issues/>
                        <div class="pager">
                            <button class="button" disabled=!has_prev on:click=move |_| start.update(|s| *s = (*s - PAGE).max(0))>"Previous"</button>
                            <button class="button" disabled=!has_next on:click=move |_| start.update(|s| *s += PAGE)>"Next"</button>
                        </div>
                    }.into_any()
                }
                Err(error) => view! { <p class="error-text" role="alert">{error.message}</p> }.into_any(),
            })}
        </Transition>
    }
}

/// `/issues?jql=...`: search with JQL and save the query as a filter.
#[component]
pub fn SearchPage() -> impl IntoView {
    let query = use_query_map();
    let navigate = use_navigate();
    let from_url = move || {
        query
            .with(|q| q.get("jql"))
            .unwrap_or_else(|| "assignee = currentUser() ORDER BY updated DESC".into())
    };
    let draft = RwSignal::new(from_url());
    let jql = Signal::derive(from_url);
    Effect::new(move |_| draft.set(jql.get()));
    let run = {
        let navigate = navigate.clone();
        move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();
            navigate(
                &format!("/issues?jql={}", api::encode(&draft.get())),
                Default::default(),
            );
        }
    };
    let saving = RwSignal::new(false);
    let filter_name = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let saved = RwSignal::new(None::<String>);
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({ "name": filter_name.get(), "jql": jql.get(), "favourite": true });
        spawn_local(async move {
            match api::post::<Filter>(&format!("{}/filter", api::CORE), &body).await {
                Ok(filter) => {
                    saved.set(Some(filter.name));
                    saving.set(false);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <div class="page">
            <header class="page-header">
                <h1>"Search"</h1>
                <button class="button" on:click=move |_| saving.set(true)>"Save as filter"</button>
            </header>
            <form class="jql-bar" on:submit=run>
                <textarea class="input jql" rows="2" spellcheck="false" aria-label="JQL query"
                    prop:value=move || draft.get()
                    on:input:target=move |ev| draft.set(ev.target().value())></textarea>
                <button class="button primary" type="submit">"Search"</button>
            </form>
            <Show when=move || saving.get()>
                <form class="inline-form" on:submit=save>
                    <input class="input" placeholder="Filter name" required
                        prop:value=move || filter_name.get()
                        on:input:target=move |ev| filter_name.set(ev.target().value())/>
                    <button class="button primary" type="submit">"Save"</button>
                    <button class="button subtle" type="button" on:click=move |_| saving.set(false)>"Cancel"</button>
                </form>
            </Show>
            <ErrorText error=error/>
            {move || saved.get().map(|name| view! { <p class="notice">{format!("Saved filter \"{name}\".")}</p> })}
            <Results jql=jql/>
        </div>
    }
}

/// `/projects/:key/issues`: every issue in the project.
#[component]
pub fn ProjectIssues() -> impl IntoView {
    let project = use_project();
    let jql =
        Signal::derive(move || format!("project = {} ORDER BY created DESC", project.key.get()));
    view! {
        <div class="page">
            <header class="page-header">
                <h1>"Issues"</h1>
                <A href=move || format!("/issues?jql={}", api::encode(&jql.get())) attr:class="button">"Open in search"</A>
            </header>
            <Results jql=jql/>
        </div>
    }
}
