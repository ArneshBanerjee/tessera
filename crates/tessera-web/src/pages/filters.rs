//! `/filters`: saved filters the user can see, with favourites.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;

use crate::api;
use crate::app::use_session;
use crate::components::Avatar;
use crate::model::{Filter, Page};

#[component]
pub fn FiltersPage() -> impl IntoView {
    let session = use_session();
    let filters = LocalResource::new(move || {
        session.changes.track();
        async {
            api::get::<Page<Filter>>(&format!("{}/filter/search?maxResults=100", api::CORE))
                .await
                .map(|page| page.values)
                .unwrap_or_default()
        }
    });
    view! {
        <div class="page">
            <header class="page-header">
                <h1>"Filters"</h1>
                <A href="/issues" attr:class="button primary">"New search"</A>
            </header>
            <Transition fallback=|| view! { <p class="muted">"Loading filters…"</p> }>
                {move || {
                    let list = filters.get().unwrap_or_default();
                    if list.is_empty() {
                        return view! { <p class="empty">"No saved filters yet. Run a search and save it."</p> }.into_any();
                    }
                    view! {
                        <table class="issue-table">
                            <thead><tr><th></th><th>"Name"</th><th>"Owner"</th><th>"Query"</th></tr></thead>
                            <tbody>
                                {list.into_iter().map(|filter| {
                                    let href = format!("/issues?jql={}", api::encode(&filter.jql));
                                    let id = filter.id.clone();
                                    let favourite = filter.favourite;
                                    let owner = filter.owner.clone();
                                    let owner_name = owner.as_ref().map(|o| o.display_name.clone()).unwrap_or_default();
                                    let toggle = move |_| {
                                        let path = format!("{}/filter/{id}/favourite", api::CORE);
                                        let method = if favourite { gloo_net::http::Method::DELETE } else { gloo_net::http::Method::PUT };
                                        spawn_local(async move {
                                            if api::request(method, &path, None).await.is_ok() {
                                                session.changed();
                                            }
                                        });
                                    };
                                    view! {
                                        <tr>
                                            <td>
                                                <button class="star" class:on=favourite on:click=toggle
                                                    title=if favourite { "Remove from favourites" } else { "Add to favourites" }>
                                                    {if favourite { "★" } else { "☆" }}
                                                </button>
                                            </td>
                                            <td><A href=href attr:class="summary-link">{filter.name}</A></td>
                                            <td><span class="person"><Avatar user=owner size=20/>{owner_name}</span></td>
                                            <td><code class="muted">{filter.jql}</code></td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_any()
                }}
            </Transition>
        </div>
    }
}
