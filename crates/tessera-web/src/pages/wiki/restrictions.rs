//! The page restrictions dialog: who may view and who may edit one page.
//!
//! As in Confluence, a page is open to the space, open for viewing but limited for editing, or
//! limited for both. The person saving is always on the list as an editor, so nobody locks
//! themselves out.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use super::space_settings::{AddSubject, Subject};
use crate::api;
use crate::app::use_session;
use crate::components::{ErrorText, Modal};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Open,
    EditLimited,
    ViewLimited,
}

impl Mode {
    fn from_key(key: &str) -> Mode {
        match key {
            "edit" => Mode::EditLimited,
            "view" => Mode::ViewLimited,
            _ => Mode::Open,
        }
    }
}

/// One person or group in the list, and whether they may edit.
#[derive(Clone, PartialEq)]
struct Entry {
    subject: Subject,
    can_edit: bool,
}

/// The subjects of one operation, as `(type, identifier, label)`.
fn subjects_of(operation: &Value) -> Vec<Subject> {
    let users = operation["restrictions"]["user"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|u| {
            (
                "user".to_string(),
                u["accountId"].as_str().unwrap_or_default().to_string(),
                u["displayName"].as_str().unwrap_or_default().to_string(),
            )
        });
    let groups = operation["restrictions"]["group"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| {
            let name = g["name"].as_str().unwrap_or_default().to_string();
            ("group".to_string(), name.clone(), format!("{name} (group)"))
        });
    users.chain(groups).collect()
}

/// The dialog's starting state from `byOperation`.
fn state_of(by_operation: &Value) -> (Mode, Vec<Entry>) {
    let viewers = subjects_of(&by_operation["read"]);
    let editors = subjects_of(&by_operation["update"]);
    let same = |a: &Subject, b: &Subject| a.0 == b.0 && a.1 == b.1;
    if viewers.is_empty() {
        let mode = if editors.is_empty() {
            Mode::Open
        } else {
            Mode::EditLimited
        };
        let entries = editors
            .into_iter()
            .map(|subject| Entry {
                subject,
                can_edit: true,
            })
            .collect();
        return (mode, entries);
    }
    // With view restrictions, everyone listed may view; editors are those also on the edit
    // list, or everyone when editing is not limited further.
    let entries = viewers
        .into_iter()
        .map(|subject| Entry {
            can_edit: editors.is_empty() || editors.iter().any(|e| same(e, &subject)),
            subject,
        })
        .collect();
    (Mode::ViewLimited, entries)
}

fn subject_json(entries: &[&Entry]) -> Value {
    let users: Vec<Value> = entries
        .iter()
        .filter(|e| e.subject.0 == "user")
        .map(|e| json!({ "type": "known", "accountId": e.subject.1 }))
        .collect();
    let groups: Vec<Value> = entries
        .iter()
        .filter(|e| e.subject.0 == "group")
        .map(|e| json!({ "type": "group", "name": e.subject.1 }))
        .collect();
    json!({ "user": users, "group": groups })
}

/// The request body that sets both operations for a mode and a list.
fn body_of(mode: Mode, entries: &[Entry]) -> Value {
    let all: Vec<&Entry> = entries.iter().collect();
    let editors: Vec<&Entry> = entries.iter().filter(|e| e.can_edit).collect();
    let (read, update) = match mode {
        Mode::Open => (vec![], vec![]),
        Mode::EditLimited => (vec![], all),
        Mode::ViewLimited => (all, editors),
    };
    json!([
        { "operation": "read", "restrictions": subject_json(&read) },
        { "operation": "update", "restrictions": subject_json(&update) },
    ])
}

/// The page header button, which shows whether the page is restricted and opens the dialog.
#[component]
pub fn RestrictionsButton(id: String) -> impl IntoView {
    let path = StoredValue::new(format!(
        "{}/content/{id}/restriction/byOperation",
        api::WIKI
    ));
    let open = RwSignal::new(false);
    let loaded = RwSignal::new(None::<(Mode, Vec<Entry>)>);
    let load = move || {
        spawn_local(async move {
            if let Ok(found) = api::get::<Value>(&path.get_value()).await {
                loaded.set(Some(state_of(&found)));
            }
        });
    };
    load();
    let label = move || match loaded.get().map(|(mode, _)| mode) {
        Some(Mode::Open) | None => "Restrictions",
        Some(Mode::EditLimited) => "Edit restricted",
        Some(Mode::ViewLimited) => "Restricted",
    };
    view! {
        <button class="button subtle" on:click=move |_| open.set(true)>{label}</button>
        {move || {
            let (mode, entries) = loaded.get()?;
            open.get().then(|| {
                view! {
                    <RestrictionsDialog id=id.clone() mode=mode entries=entries
                        on_close=Callback::new(move |saved: bool| {
                            open.set(false);
                            if saved {
                                load();
                            }
                        })/>
                }
            })
        }}
    }
}

#[component]
fn RestrictionsDialog(
    id: String,
    mode: Mode,
    entries: Vec<Entry>,
    /// Called with whether the restrictions were saved.
    on_close: Callback<bool>,
) -> impl IntoView {
    let me = use_session().me.get_value();
    let mine = StoredValue::new(me.account_id.clone());
    let mut entries = entries;
    // As in Confluence, you are always on the list, as an editor.
    entries.retain(|e| !(e.subject.0 == "user" && e.subject.1 == me.account_id));
    entries.insert(
        0,
        Entry {
            subject: (
                "user".into(),
                me.account_id,
                format!("{} (you)", me.display_name),
            ),
            can_edit: true,
        },
    );
    let mode = RwSignal::new(mode);
    let entries = RwSignal::new(entries);
    let error = RwSignal::new(None::<String>);
    let id = StoredValue::new(id);
    let add = Callback::new(move |subject: Subject| {
        entries.update(|list| {
            if !list
                .iter()
                .any(|e| e.subject.0 == subject.0 && e.subject.1 == subject.1)
            {
                list.push(Entry {
                    subject,
                    can_edit: true,
                });
            }
        });
    });
    let save = move |_| {
        let body = body_of(mode.get(), &entries.get());
        spawn_local(async move {
            let path = format!("{}/content/{}/restriction", api::WIKI, id.get_value());
            match api::put::<Value>(&path, &body).await {
                Ok(_) => on_close.run(true),
                Err(e) => error.set(Some(e.message)),
            }
        });
    };
    view! {
        <Modal title="Restrictions".to_string() on_close=Callback::new(move |()| on_close.run(false))>
            <div class="form restrictions">
                <select class="select" aria-label="Who can view and edit"
                    on:change:target=move |ev| mode.set(Mode::from_key(&ev.target().value()))>
                    <option value="open" selected=mode.get_untracked() == Mode::Open>"Anyone can view and edit"</option>
                    <option value="edit" selected=mode.get_untracked() == Mode::EditLimited>"Anyone can view, only some can edit"</option>
                    <option value="view" selected=mode.get_untracked() == Mode::ViewLimited>"Only specific people can view or edit"</option>
                </select>
                <Show when=move || mode.get() != Mode::Open
                    fallback=|| view! { <p class="muted small">"Everyone who can see this space can view and edit this page."</p> }>
                    <p class="muted small">{move || match mode.get() {
                        Mode::EditLimited => "Everyone in the space can view. Only these people and groups can edit.",
                        _ => "Only these people and groups can view this page and the pages below it.",
                    }}</p>
                    <table class="issue-table">
                        <tbody>
                            <For each=move || entries.get() key=|e| (e.subject.0.clone(), e.subject.1.clone()) let:entry>
                                {
                                    let is_me = entry.subject.0 == "user" && entry.subject.1 == mine.get_value();
                                    let key = (entry.subject.0.clone(), entry.subject.1.clone());
                                    let same = move |e: &Entry| e.subject.0 == key.0 && e.subject.1 == key.1;
                                    let same_for_change = same.clone();
                                    view! {
                                        <tr>
                                            <td>{entry.subject.2.clone()}</td>
                                            <td>
                                                <Show when=move || mode.get() == Mode::ViewLimited && !is_me
                                                    fallback=|| view! { <span class="muted small">"Can edit"</span> }>
                                                    <select class="select" aria-label="Permission"
                                                        on:change:target={
                                                            let same = same_for_change.clone();
                                                            move |ev| {
                                                                let edit = ev.target().value() == "edit";
                                                                entries.update(|list| {
                                                                    if let Some(e) = list.iter_mut().find(|e| same(e)) {
                                                                        e.can_edit = edit;
                                                                    }
                                                                });
                                                            }
                                                        }>
                                                        <option value="view" selected=!entry.can_edit>"Can view"</option>
                                                        <option value="edit" selected=entry.can_edit>"Can edit"</option>
                                                    </select>
                                                </Show>
                                            </td>
                                            <td>
                                                {(!is_me).then(|| view! {
                                                    <button class="link-button small" on:click={
                                                        let same = same.clone();
                                                        move |_| entries.update(|list| list.retain(|e| !same(e)))
                                                    }>"Remove"</button>
                                                })}
                                            </td>
                                        </tr>
                                    }
                                }
                            </For>
                        </tbody>
                    </table>
                    <AddSubject on_add=add/>
                </Show>
                <ErrorText error=error/>
                <div class="form-actions">
                    <button class="button primary" on:click=save>"Apply"</button>
                    <button class="button subtle" on:click=move |_| on_close.run(false)>"Cancel"</button>
                </div>
            </div>
        </Modal>
    }
}
