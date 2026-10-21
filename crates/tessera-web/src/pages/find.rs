//! Search across Jira and Confluence: the box in the top bar, with results as you type, and
//! `/search?q=`, every section in full.

use std::time::Duration;

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_query_map};
use serde::Deserialize;

use crate::api;
use crate::model::short_time;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Found {
    #[serde(rename = "type")]
    kind: String,
    title: String,
    subtitle: String,
    link: String,
    icon: Option<String>,
    updated: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Section {
    key: String,
    label: String,
    results: Vec<Found>,
    more: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Results {
    sections: Vec<Section>,
}

async fn search(text: &str, limit: usize) -> Vec<Section> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    api::get::<Results>(&format!(
        "{}/search?query={}&limit={limit}",
        api::ACCOUNT,
        api::encode(text.trim())
    ))
    .await
    .map(|r| r.sections)
    .unwrap_or_default()
}

/// Whether text looks like an issue key, such as PROJ-12.
fn is_key(text: &str) -> bool {
    text.rsplit_once('-').is_some_and(|(project, number)| {
        !project.is_empty() && !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
    })
}

/// A short glyph for results without an icon.
fn glyph(kind: &str) -> &'static str {
    match kind {
        "page" => "📄",
        "blogpost" => "📰",
        "space" => "🗂",
        "project" => "📁",
        "filter" => "🔎",
        "dashboard" => "📊",
        _ => "•",
    }
}

#[component]
fn ResultRow(found: Found, on_pick: Callback<()>) -> impl IntoView {
    let icon = found.icon.clone();
    view! {
        <A href=found.link.clone() attr:class="find-row" on:click=move |_| on_pick.run(())>
            {match icon {
                Some(src) => view! { <img class="icon16" src=src alt=""/> }.into_any(),
                None => view! { <span class="find-glyph" aria-hidden="true">{glyph(&found.kind)}</span> }.into_any(),
            }}
            <span class="find-text">
                <span class="find-title">{found.title.clone()}</span>
                <span class="muted small">{found.subtitle.clone()}</span>
            </span>
            {found.updated.clone().map(|when| view! { <span class="muted small find-when">{short_time(&when)}</span> })}
        </A>
    }
}

/// The search box in the top bar. Results show as you type; Enter opens an issue by key, or
/// every result.
#[component]
pub fn SearchBox() -> impl IntoView {
    let navigate = use_navigate();
    let text = RwSignal::new(String::new());
    let open = RwSignal::new(false);
    // Searches once typing pauses, and ignores answers to older text.
    let asked = RwSignal::new(String::new());
    let results = LocalResource::new(move || {
        let wanted = asked.get();
        async move { search(&wanted, 4).await }
    });
    let on_input = move |value: String| {
        text.set(value.clone());
        open.set(!value.trim().is_empty());
        set_timeout(
            move || {
                if text.get_untracked() == value {
                    asked.set(value);
                }
            },
            Duration::from_millis(250),
        );
    };
    let close = Callback::new(move |()| {
        open.set(false);
        text.set(String::new());
    });
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let value = text.get().trim().to_string();
        if value.is_empty() {
            return;
        }
        let target = if is_key(&value) {
            format!("/browse/{}", value.to_uppercase())
        } else {
            format!("/search?q={}", api::encode(&value))
        };
        close.run(());
        navigate(&target, Default::default());
    };
    view! {
        <form class="nav-search" role="search" on:submit=submit>
            <input class="input" type="search" placeholder="Search Jira and Confluence, or go to PROJ-1"
                aria-label="Search"
                prop:value=move || text.get()
                on:input:target=move |ev| on_input(ev.target().value())
                on:keydown=move |ev| if ev.key() == "Escape" { open.set(false); }
                on:blur=move |_| { set_timeout(move || open.set(false), Duration::from_millis(200)); }/>
            <Show when=move || open.get()>
                <div class="menu find-menu" role="listbox" aria-label="Search results">
                    {move || {
                        let sections: Vec<Section> = results.get().unwrap_or_default().into_iter().filter(|s| !s.results.is_empty()).collect();
                        let all = format!("/search?q={}", api::encode(text.get().trim()));
                        if sections.is_empty() {
                            return view! {
                                <p class="muted small find-empty">"Press Enter to search everything."</p>
                            }.into_any();
                        }
                        view! {
                            {sections.into_iter().map(|section| view! {
                                <div class="find-section">
                                    <div class="find-label">{section.label.clone()}</div>
                                    {section.results.into_iter().map(|found| view! { <ResultRow found=found on_pick=close/> }).collect_view()}
                                </div>
                            }).collect_view()}
                            <A href=all attr:class="find-all" on:click=move |_| close.run(())>"See all results"</A>
                        }.into_any()
                    }}
                </div>
            </Show>
        </form>
    }
}

/// `/search?q=`: every section of a search.
#[component]
pub fn SearchResults() -> impl IntoView {
    let query = use_query_map();
    let text = Signal::derive(move || query.with(|q| q.get("q")).unwrap_or_default());
    let results = LocalResource::new(move || {
        let wanted = text.get();
        async move { search(&wanted, 20).await }
    });
    view! {
        <div class="page search-results-page">
            <header class="page-header">
                <h1>{move || format!("Results for “{}”", text.get())}</h1>
            </header>
            <Transition fallback=|| view! { <p class="muted">"Searching…"</p> }>
                {move || results.get().map(|sections| {
                    let any = sections.iter().any(|s| !s.results.is_empty());
                    if !any {
                        return view! { <p class="empty">"Nothing matches. Try other words."</p> }.into_any();
                    }
                    sections.into_iter().filter(|s| !s.results.is_empty()).map(|section| {
                        let more = section.more.clone().filter(|_| section.key == "issues");
                        view! {
                            <section class="settings-card find-block">
                                <div class="card-heading">
                                    <h2>{section.label.clone()}</h2>
                                    {more.map(|href| view! { <A href=href>"Open in issue search"</A> })}
                                </div>
                                {section.results.into_iter().map(|found| view! { <ResultRow found=found on_pick=Callback::new(|()| ())/> }).collect_view()}
                            </section>
                        }
                    }).collect_view().into_any()
                })}
            </Transition>
        </div>
    }
}
