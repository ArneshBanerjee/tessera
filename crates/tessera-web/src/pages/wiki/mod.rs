//! The Confluence side of the app: spaces, the page tree, pages, and the editor.

pub mod editor;
pub mod page;
mod restrictions;
pub mod space_settings;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::{A, Outlet, Redirect};
use leptos_router::hooks::{use_location, use_navigate, use_params_map};
use serde_json::{Value, json};

use crate::api;
use crate::app::use_session;
use crate::components::{ErrorText, Modal};
use crate::model::{CursorPage, TreePage, WikiSpace};

/// The space a page belongs to, provided by [`SpaceLayout`].
#[derive(Clone, Copy)]
pub struct SpaceContext {
    pub key: Signal<String>,
    pub space: LocalResource<Option<WikiSpace>>,
}

pub fn use_space() -> SpaceContext {
    expect_context::<SpaceContext>()
}

/// The path of a page in the web app.
pub fn page_path(space_key: &str, id: &str) -> String {
    format!("/wiki/spaces/{space_key}/pages/{id}")
}

/// Turns Markdown into the v2 API's ADF body.
pub fn adf_body(markdown: &str) -> Value {
    let doc = tessera_adf::from_markdown(markdown);
    json!({
        "representation": "atlas_doc_format",
        "value": serde_json::to_string(&doc).unwrap_or_default(),
    })
}

/// `/wiki`: every space.
#[component]
pub fn Spaces() -> impl IntoView {
    let session = use_session();
    let spaces = LocalResource::new(move || {
        session.changes.track();
        async {
            api::get::<CursorPage<WikiSpace>>(&format!(
                "{}/spaces?limit=250&description-format=plain",
                api::WIKI2
            ))
            .await
        }
    });
    let creating = RwSignal::new(false);
    view! {
        <div class="page">
            <header class="page-header">
                <h1>"Spaces"</h1>
                <button class="button primary" on:click=move |_| creating.set(true)>"Create space"</button>
            </header>
            <Transition fallback=|| view! { <p class="muted">"Loading spaces…"</p> }>
                {move || spaces.get().map(|result| match result {
                    Ok(page) if page.results.is_empty() => view! {
                        <p class="empty">"No spaces yet. Create one to start writing."</p>
                    }.into_any(),
                    Ok(page) => view! {
                        <div class="space-grid">
                            {page.results.into_iter().map(|space| {
                                let href = format!("/wiki/spaces/{}", space.key);
                                let description = space.description();
                                let initial = space.name.chars().next().unwrap_or('?').to_uppercase().to_string();
                                view! {
                                    <A href=href attr:class="space-card">
                                        <span class="space-icon">{initial}</span>
                                        <span class="space-card-text">
                                            <strong>{space.name}</strong>
                                            <span class="muted small">{space.key}</span>
                                            <span class="muted small">{description}</span>
                                        </span>
                                    </A>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any(),
                    Err(error) => view! { <p class="error-text">{error.message}</p> }.into_any(),
                })}
            </Transition>
            <Show when=move || creating.get()>
                <CreateSpace on_close=Callback::new(move |()| creating.set(false))/>
            </Show>
        </div>
    }
}

/// A space key from a name: the first letters of its words, or its first letters.
fn suggest_key(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let key: String = if words.len() > 1 {
        words.iter().filter_map(|w| w.chars().next()).collect()
    } else {
        name.chars().take(6).collect()
    };
    key.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase()
}

#[component]
fn CreateSpace(on_close: Callback<()>) -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let name = RwSignal::new(String::new());
    let key = RwSignal::new(String::new());
    let key_edited = RwSignal::new(false);
    let description = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = json!({
            "key": key.get(),
            "name": name.get(),
            "description": { "value": description.get(), "representation": "plain" },
        });
        let navigate = navigate.clone();
        spawn_local(async move {
            match api::post::<Value>(&format!("{}/spaces", api::WIKI2), &body).await {
                Ok(space) => {
                    session.changed();
                    on_close.run(());
                    navigate(
                        &format!("/wiki/spaces/{}", space["key"].as_str().unwrap_or_default()),
                        Default::default(),
                    );
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <Modal title="Create space".into() on_close=on_close>
            <form class="form" on:submit=submit>
                <label>"Name"
                    <input class="input" required
                        prop:value=move || name.get()
                        on:input:target=move |ev| {
                            let value = ev.target().value();
                            if !key_edited.get() {
                                key.set(suggest_key(&value));
                            }
                            name.set(value);
                        }/>
                </label>
                <label>"Key"
                    <input class="input" required
                        prop:value=move || key.get()
                        on:input:target=move |ev| {
                            key_edited.set(true);
                            key.set(ev.target().value().to_uppercase());
                        }/>
                </label>
                <label>"Description"
                    <textarea class="input" rows="3"
                        prop:value=move || description.get()
                        on:input:target=move |ev| description.set(ev.target().value())></textarea>
                </label>
                <ErrorText error=error/>
                <div class="form-actions">
                    <button class="button primary" type="submit">"Create"</button>
                </div>
            </form>
        </Modal>
    }
}

/// The frame of a space: a sidebar with the page tree, and the current page.
#[component]
pub fn SpaceLayout() -> impl IntoView {
    let session = use_session();
    let params = use_params_map();
    let key = Signal::derive(move || {
        params
            .with(|p| p.get("key"))
            .unwrap_or_default()
            .to_uppercase()
    });
    let space = LocalResource::new(move || {
        let key = key.get();
        session.changes.track();
        async move {
            api::get::<CursorPage<WikiSpace>>(&format!(
                "{}/spaces?keys={}&description-format=plain",
                api::WIKI2,
                api::encode(&key)
            ))
            .await
            .ok()
            .and_then(|page| page.results.into_iter().next())
        }
    });
    provide_context(SpaceContext { key, space });
    view! {
        <div class="wiki-layout">
            <aside class="sidebar wiki-sidebar">
                {move || space.get().flatten().map(|s| {
                    let home = s.homepage_id.clone().map(|id| page_path(&s.key, &id)).unwrap_or_default();
                    let create = format!("/wiki/spaces/{}/create", s.key);
                    let initial = s.name.chars().next().unwrap_or('?').to_uppercase().to_string();
                    view! {
                        <div class="sidebar-project">
                            <span class="space-icon small">{initial}</span>
                            <div>
                                <div class="sidebar-project-name">{s.name.clone()}</div>
                                <div class="muted small">"Space"</div>
                            </div>
                        </div>
                        <nav class="sidebar-nav">
                            <A href=home attr:class="sidebar-link">"Overview"</A>
                            <A href=format!("/wiki/spaces/{}/settings", s.key) attr:class="sidebar-link">"Space settings"</A>
                        </nav>
                        <div class="tree-header">
                            <span class="muted small">"Pages"</span>
                            <A href=create attr:class="button subtle small" attr:title="Create a page">"+"</A>
                        </div>
                        <PageTree space=s/>
                    }
                })}
            </aside>
            <section class="project-content">
                <Outlet/>
            </section>
        </div>
    }
}

/// The pages of a space as a tree. Children load when a page is opened.
#[component]
fn PageTree(space: WikiSpace) -> impl IntoView {
    let session = use_session();
    let space_id = space.id.clone();
    let roots = LocalResource::new(move || {
        let space_id = space_id.clone();
        session.changes.track();
        async move {
            api::get::<CursorPage<TreePage>>(&format!(
                "{}/spaces/{space_id}/pages?depth=root&limit=250",
                api::WIKI2
            ))
            .await
            .map(|page| page.results)
            .unwrap_or_default()
        }
    });
    let key = StoredValue::new(space.key);
    view! {
        <ul class="page-tree">
            {move || roots.get().unwrap_or_default().into_iter().map(|page| view! {
                <TreeNode page=page space_key=key.get_value() open_initially=true/>
            }).collect_view()}
        </ul>
    }
}

/// One page in the tree. It renders itself for its children, so its view type is erased.
#[component]
fn TreeNode(
    page: TreePage,
    space_key: String,
    #[prop(default = false)] open_initially: bool,
) -> AnyView {
    let session = use_session();
    let location = use_location();
    let open = RwSignal::new(open_initially);
    let id = StoredValue::new(page.id.clone());
    let children = LocalResource::new(move || {
        let id = id.get_value();
        let load = open.get();
        session.changes.track();
        async move {
            if !load {
                return Vec::new();
            }
            api::get::<CursorPage<TreePage>>(&format!(
                "{}/pages/{id}/children?limit=250",
                api::WIKI2
            ))
            .await
            .map(|page| page.results)
            .unwrap_or_default()
        }
    });
    let href = page_path(&space_key, &page.id);
    let current = {
        let href = href.clone();
        // The page itself, or its edit and history views.
        move || {
            let path = location.pathname.get();
            path == href || path.starts_with(&format!("{href}/"))
        }
    };
    let key = StoredValue::new(space_key);
    view! {
        <li class="tree-item">
            <div class="tree-row" class:current=current>
                <button class="tree-toggle" title="Show child pages"
                    on:click=move |_| open.update(|o| *o = !*o)>
                    {move || if open.get() { "▾" } else { "▸" }}
                </button>
                <A href=href attr:class="tree-link">{page.title}</A>
            </div>
            <Show when=move || open.get()>
                <ul class="page-tree">
                    {move || children.get().unwrap_or_default().into_iter().map(|child| view! {
                        <TreeNode page=child space_key=key.get_value()/>
                    }).collect_view()}
                </ul>
            </Show>
        </li>
    }
    .into_any()
}

/// `/wiki/spaces/:key`: opens the space's home page.
#[component]
pub fn SpaceHome() -> impl IntoView {
    let space = use_space();
    view! {
        <Transition fallback=|| ()>
            {move || space.space.get().map(|found| match found {
                Some(s) => match s.homepage_id {
                    Some(home) => view! { <Redirect path=format!("pages/{home}")/> }.into_any(),
                    None => view! { <div class="page"><p class="empty">"This space has no home page."</p></div> }.into_any(),
                },
                None => view! { <div class="page"><h1>"Space not found"</h1></div> }.into_any(),
            })}
        </Transition>
    }
}
