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

/// The `@name` being typed just before the caret: the byte offset of its `@`, and the name
/// typed so far. `caret` counts UTF-16 units, as the browser does.
fn mention_query(text: &str, caret: usize) -> Option<(usize, String)> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let prefix = String::from_utf16_lossy(&units[..caret.min(units.len())]);
    let at = prefix.rfind('@')?;
    let typed = &prefix[at + 1..];
    let starts_word = prefix[..at]
        .chars()
        .last()
        .is_none_or(|c| c.is_whitespace() || c == '(');
    let plain = typed.chars().count() <= 30 && !typed.contains(['\n', '[', ']', '(', ')']);
    // Names have spaces, so allow them, but not two in a row or a leading one.
    let words_ok = !typed.starts_with(' ') && !typed.contains("  ");
    (starts_word && plain && words_ok).then(|| (at, typed.to_string()))
}

/// A Markdown text box. Typing `@` and part of a name offers people to mention; picking one
/// writes `[@Name](mention:accountId)`, which saves as a mention.
#[component]
pub fn MarkdownArea(
    value: RwSignal<String>,
    #[prop(default = 4)] rows: u32,
    #[prop(into, optional)] placeholder: String,
    #[prop(into, default = "input".to_string())] class: String,
    #[prop(optional)] autofocus: bool,
) -> impl IntoView {
    let node: NodeRef<leptos::html::Textarea> = NodeRef::new();
    if autofocus {
        Effect::new(move |_| {
            if let Some(element) = node.get() {
                let _ = element.focus();
            }
        });
    }
    let users = users_resource();
    let query = RwSignal::new(None::<(usize, String)>);
    let active = RwSignal::new(0usize);
    let matches = move || -> Vec<User> {
        let Some((_, typed)) = query.get() else {
            return Vec::new();
        };
        let typed = typed.to_lowercase();
        users
            .get()
            .unwrap_or_default()
            .into_iter()
            .filter(|u| {
                u.display_name.to_lowercase().contains(&typed)
                    || u.email_address.to_lowercase().starts_with(&typed)
            })
            .take(6)
            .collect()
    };
    let update_query = move || {
        if let Some(element) = node.get_untracked() {
            let caret = element.selection_start().ok().flatten().unwrap_or(0) as usize;
            query.set(mention_query(&element.value(), caret));
            active.set(0);
        }
    };
    let pick = move |user: User| {
        let (Some(element), Some((start, _))) = (node.get_untracked(), query.get_untracked())
        else {
            return;
        };
        let text = element.value();
        let caret = element.selection_start().ok().flatten().unwrap_or(0) as usize;
        let units: Vec<u16> = text.encode_utf16().collect();
        let end = String::from_utf16_lossy(&units[..caret.min(units.len())]).len();
        let name: String = user
            .display_name
            .chars()
            .filter(|c| !matches!(c, '[' | ']'))
            .collect();
        let token = format!("[@{name}](mention:{}) ", user.account_id);
        let updated = format!("{}{token}{}", &text[..start], &text[end..]);
        element.set_value(&updated);
        value.set(updated.clone());
        let position = (text[..start].encode_utf16().count() + token.encode_utf16().count()) as u32;
        let _ = element.set_selection_range(position, position);
        let _ = element.focus();
        query.set(None);
    };
    let on_key = move |ev: leptos::ev::KeyboardEvent| {
        let list = matches();
        if list.is_empty() {
            return;
        }
        match ev.key().as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                active.update(|a| *a = (*a + 1) % list.len());
            }
            "ArrowUp" => {
                ev.prevent_default();
                active.update(|a| *a = (*a + list.len() - 1) % list.len());
            }
            "Enter" | "Tab" => {
                ev.prevent_default();
                let chosen = list[active.get_untracked().min(list.len() - 1)].clone();
                pick(chosen);
            }
            "Escape" => {
                ev.prevent_default();
                query.set(None);
            }
            _ => {}
        }
    };
    view! {
        <div class="markdown-area">
            <textarea class=class rows=rows placeholder=placeholder node_ref=node
                prop:value=move || value.get()
                on:input:target=move |ev| {
                    value.set(ev.target().value());
                    update_query();
                }
                on:keydown=on_key
                on:click=move |_| update_query()
                on:blur=move |_| query.set(None)></textarea>
            {move || {
                let list = matches();
                (!list.is_empty()).then(|| view! {
                    <ul class="mention-menu" role="listbox" aria-label="People to mention">
                        {list.into_iter().enumerate().map(|(i, user)| {
                            let chosen = user.clone();
                            view! {
                                <li role="option" class="mention-option" class:active=move || active.get() == i
                                    aria-selected=move || (active.get() == i).to_string()
                                    // Mouse down, not click: the text box keeps its focus.
                                    on:mousedown=move |ev| { ev.prevent_default(); pick(chosen.clone()); }>
                                    <Avatar user=Some(user.clone()) size=20/>
                                    <span>{user.display_name.clone()}</span>
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                })
            }}
        </div>
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
