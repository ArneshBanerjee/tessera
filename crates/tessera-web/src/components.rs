//! Small shared pieces of UI.

use leptos::html::ElementType;
use leptos::prelude::*;
use serde_json::Value;
use wasm_bindgen::JsCast;

use crate::api;
use crate::model::{Named, Status, User};

#[component]
pub fn Avatar(user: Option<User>, #[prop(default = 24)] size: u32) -> impl IntoView {
    let (src, title) = match &user {
        Some(user) => (user.avatar(), user.display_name.clone()),
        None => (String::new(), "Unassigned".to_string()),
    };
    let style = format!("width:{size}px;height:{size}px");
    if src.is_empty() {
        view! { <span class="avatar avatar-empty" style=style title=title></span> }.into_any()
    } else {
        view! { <img class="avatar" src=src style=style title=title.clone() alt=title/> }.into_any()
    }
}

#[component]
pub fn TypeIcon(item: Option<Named>) -> impl IntoView {
    item.map(|item| view! { <img class="icon16" src=item.icon_url title=item.name.clone() alt=item.name/> })
}

/// A status shown as a colored label, colored by its category.
#[component]
pub fn Lozenge(status: Option<Status>) -> impl IntoView {
    status.map(|status| {
        let class = format!("lozenge lozenge-{}", status.status_category.key);
        view! { <span class=class>{status.name}</span> }
    })
}

/// A dialog over the page. Clicking the backdrop or pressing Close calls `on_close`.
#[component]
pub fn Modal(title: String, on_close: Callback<()>, children: Children) -> impl IntoView {
    let label = title.clone();
    view! {
        <div class="backdrop" on:click=move |_| on_close.run(())>
            <div class="modal" role="dialog" aria-label=label on:click=|ev| ev.stop_propagation()>
                <header class="modal-header">
                    <h2>{title}</h2>
                    <button class="button subtle" on:click=move |_| on_close.run(())>"Close"</button>
                </header>
                <div class="modal-body">{children()}</div>
            </div>
        </div>
    }
}

/// A node reference that focuses its element once it is on the page.
///
/// The `autofocus` attribute only works on page load, not on elements added later.
pub fn autofocus<E>() -> NodeRef<E>
where
    E: ElementType,
    E::Output: JsCast + Clone + 'static,
{
    let node = NodeRef::new();
    Effect::new(move |_| {
        let element: Option<E::Output> = node.get();
        if let Some(element) = element {
            let _ = element.unchecked_ref::<web_sys::HtmlElement>().focus();
        }
    });
    node
}

/// An error message, when there is one.
#[component]
pub fn ErrorText(error: RwSignal<Option<String>>) -> impl IntoView {
    move || {
        error
            .get()
            .map(|message| view! { <p class="error-text" role="alert">{message}</p> })
    }
}

/// All active users, loaded once per page that needs a user picker.
pub fn users_resource() -> LocalResource<Vec<User>> {
    LocalResource::new(|| async {
        api::get::<Vec<User>>(&format!("{}/users/search?maxResults=1000", api::CORE))
            .await
            .unwrap_or_default()
    })
}

/// A select list of users, with "Unassigned" first.
#[component]
pub fn UserSelect(
    users: LocalResource<Vec<User>>,
    selected: Option<String>,
    on_change: Callback<Option<String>>,
) -> impl IntoView {
    view! {
        <select
            class="select"
            on:change:target=move |ev| {
                let value = ev.target().value();
                on_change.run((!value.is_empty()).then_some(value));
            }
        >
            <option value="" selected=selected.is_none()>"Unassigned"</option>
            {move || {
                let selected = selected.clone();
                users
                    .get()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|user| {
                        let is_selected = selected.as_deref() == Some(user.account_id.as_str());
                        view! {
                            <option value=user.account_id selected=is_selected>{user.display_name}</option>
                        }
                    })
                    .collect_view()
            }}
        </select>
    }
}

/// Renders an ADF document. Text is always inserted as text, never as HTML.
pub fn render_adf(doc: &Value) -> AnyView {
    if doc.is_null() {
        return view! { <p class="muted">"No description."</p> }.into_any();
    }
    view! { <div class="adf">{children(doc)}</div> }.into_any()
}

fn children(node: &Value) -> AnyView {
    node["content"]
        .as_array()
        .map(|items| items.iter().map(render_node).collect_view().into_any())
        .unwrap_or_else(|| ().into_any())
}

fn render_node(node: &Value) -> AnyView {
    let text = || node["text"].as_str().unwrap_or_default().to_string();
    match node["type"].as_str().unwrap_or_default() {
        "paragraph" => view! { <p>{children(node)}</p> }.into_any(),
        "heading" => match node["attrs"]["level"].as_u64().unwrap_or(1) {
            1 => view! { <h1>{children(node)}</h1> }.into_any(),
            2 => view! { <h2>{children(node)}</h2> }.into_any(),
            3 => view! { <h3>{children(node)}</h3> }.into_any(),
            4 => view! { <h4>{children(node)}</h4> }.into_any(),
            5 => view! { <h5>{children(node)}</h5> }.into_any(),
            _ => view! { <h6>{children(node)}</h6> }.into_any(),
        },
        "bulletList" => view! { <ul>{children(node)}</ul> }.into_any(),
        "orderedList" => view! { <ol>{children(node)}</ol> }.into_any(),
        "listItem" => view! { <li>{children(node)}</li> }.into_any(),
        "blockquote" => view! { <blockquote>{children(node)}</blockquote> }.into_any(),
        "codeBlock" => {
            let code: String = node["content"]
                .as_array()
                .map(|parts| parts.iter().filter_map(|p| p["text"].as_str()).collect())
                .unwrap_or_default();
            view! { <pre><code>{code}</code></pre> }.into_any()
        }
        "rule" => view! { <hr/> }.into_any(),
        "hardBreak" => view! { <br/> }.into_any(),
        "panel" => view! { <div class="panel">{children(node)}</div> }.into_any(),
        "mention" | "emoji" | "status" => {
            let label = node["attrs"]["text"]
                .as_str()
                .or_else(|| node["attrs"]["shortName"].as_str())
                .unwrap_or_default()
                .to_string();
            view! { <span class="mention">{label}</span> }.into_any()
        }
        "text" => {
            let mut view = text().into_any();
            for mark in node["marks"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                view = match mark["type"].as_str().unwrap_or_default() {
                    "strong" => view! { <strong>{view}</strong> }.into_any(),
                    "em" => view! { <em>{view}</em> }.into_any(),
                    "strike" => view! { <s>{view}</s> }.into_any(),
                    "code" => view! { <code>{view}</code> }.into_any(),
                    "underline" => view! { <u>{view}</u> }.into_any(),
                    "link" => {
                        let href = mark["attrs"]["href"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        // Only web and mail links become clickable.
                        if href.starts_with("http://")
                            || href.starts_with("https://")
                            || href.starts_with("mailto:")
                        {
                            view! { <a href=href target="_blank" rel="noopener noreferrer">{view}</a> }.into_any()
                        } else {
                            view
                        }
                    }
                    _ => view,
                };
            }
            view
        }
        _ => children(node),
    }
}
