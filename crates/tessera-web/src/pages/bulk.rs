//! Bulk change: edit, transition, move, watch, or delete the issues picked in a search.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Value, json};

use crate::api;
use crate::app::use_session;
use crate::components::{ErrorText, Modal, UserSelect, users_resource};
use crate::model::{Named, Project};

#[derive(Clone, Copy, PartialEq)]
enum Operation {
    Edit,
    Transition,
    Move,
    Watch,
    Delete,
}

const OPERATIONS: &[(Operation, &str)] = &[
    (Operation::Edit, "Edit fields"),
    (Operation::Transition, "Transition"),
    (Operation::Move, "Move"),
    (Operation::Watch, "Watch"),
    (Operation::Delete, "Delete"),
];

/// Runs a bulk operation and reads back what it did, in words.
async fn run(path: &str, body: Value) -> api::ApiResult<String> {
    let created: Value = api::post(&format!("{}/bulk/{path}", api::CORE), &body).await?;
    let task = created["taskId"].as_str().unwrap_or_default();
    let progress: Value = api::get(&format!("{}/bulk/queue/{task}", api::CORE)).await?;
    let done = progress["processedAccessibleIssues"]
        .as_array()
        .map_or(0, Vec::len);
    let total = progress["totalIssueCount"].as_i64().unwrap_or_default();
    let mut summary = format!("Changed {done} of {total} issues.");
    if let Some(failed) = progress["failedAccessibleIssues"].as_object()
        && !failed.is_empty()
    {
        for (id, reasons) in failed {
            let reasons: Vec<&str> = reasons
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            summary.push_str(&format!(" Issue {id}: {}.", reasons.join(" ")));
        }
    }
    let invalid = progress["invalidOrInaccessibleIssueCount"]
        .as_i64()
        .unwrap_or_default();
    if invalid > 0 {
        summary.push_str(&format!(
            " {invalid} could not be found or are not visible to you."
        ));
    }
    Ok(summary)
}

#[component]
pub fn BulkChange(keys: Vec<String>, on_close: Callback<bool>) -> impl IntoView {
    let session = use_session();
    let operation = RwSignal::new(Operation::Edit);
    let error = RwSignal::new(None::<String>);
    let result = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let keys = StoredValue::new(keys);
    let count = keys.with_value(Vec::len);
    let submit = Callback::new(move |(path, body): (String, Value)| {
        busy.set(true);
        spawn_local(async move {
            match run(&path, body).await {
                Ok(summary) => {
                    result.set(Some(summary));
                    error.set(None);
                    session.changed();
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    });
    view! {
        <Modal title=format!("Bulk change {count} issues") on_close=Callback::new(move |()| on_close.run(result.get_untracked().is_some()))>
            {move || match result.get() {
                Some(summary) => view! {
                    <div class="form">
                        <p role="status">{summary}</p>
                        <div class="form-actions">
                            <button class="button primary" on:click=move |_| on_close.run(true)>"Done"</button>
                        </div>
                    </div>
                }.into_any(),
                None => view! {
                    <div class="bulk-change">
                        <nav class="tabs">
                            {OPERATIONS.iter().map(|(op, label)| {
                                let op = *op;
                                view! {
                                    <button class="tab" class:active=move || operation.get() == op
                                        on:click=move |_| { operation.set(op); error.set(None); }>{*label}</button>
                                }
                            }).collect_view()}
                        </nav>
                        {move || match operation.get() {
                            Operation::Edit => view! { <EditForm keys=keys.get_value() submit=submit busy=busy/> }.into_any(),
                            Operation::Transition => view! { <TransitionForm keys=keys.get_value() submit=submit busy=busy/> }.into_any(),
                            Operation::Move => view! { <MoveForm keys=keys.get_value() submit=submit busy=busy/> }.into_any(),
                            Operation::Watch => {
                                let watch = move |path: &str| {
                                    submit.run((path.to_string(), json!({ "selectedIssueIdsOrKeys": keys.get_value() })));
                                };
                                view! {
                                    <div class="form">
                                        <p class="muted small">"Watching an issue sends you its notifications."</p>
                                        <div class="form-actions">
                                            <button class="button primary" disabled=move || busy.get() on:click=move |_| watch("issues/watch")>"Watch all"</button>
                                            <button class="button" disabled=move || busy.get() on:click=move |_| watch("issues/unwatch")>"Stop watching all"</button>
                                        </div>
                                    </div>
                                }.into_any()
                            }
                            Operation::Delete => view! {
                                <div class="form">
                                    <p>{format!("Delete {count} issues and their subtasks? This cannot be undone.")}</p>
                                    <div class="form-actions">
                                        <button class="button danger" disabled=move || busy.get()
                                            on:click=move |_| submit.run(("issues/delete".into(), json!({ "selectedIssueIdsOrKeys": keys.get_value() })))>
                                            {format!("Delete {count} issues")}
                                        </button>
                                    </div>
                                </div>
                            }.into_any(),
                        }}
                        <ErrorText error=error/>
                    </div>
                }.into_any(),
            }}
        </Modal>
    }
}

#[component]
fn EditForm(
    keys: Vec<String>,
    submit: Callback<(String, Value)>,
    busy: RwSignal<bool>,
) -> impl IntoView {
    let users = users_resource();
    let priorities = LocalResource::new(|| async {
        api::get::<Vec<Named>>(&format!("{}/priority", api::CORE))
            .await
            .unwrap_or_default()
    });
    let change_assignee = RwSignal::new(false);
    let assignee = RwSignal::new(None::<String>);
    let change_priority = RwSignal::new(false);
    let priority = RwSignal::new(String::new());
    let change_labels = RwSignal::new(false);
    let labels = RwSignal::new(String::new());
    let label_mode = RwSignal::new("ADD".to_string());
    let change_due = RwSignal::new(false);
    let due = RwSignal::new(String::new());
    let keys = StoredValue::new(keys);
    let apply = move |_| {
        let mut actions = Vec::new();
        let mut input = json!({});
        if change_assignee.get() {
            actions.push("assignee");
            let user = assignee
                .get()
                .map(|id| json!({ "accountId": id }))
                .unwrap_or(Value::Null);
            input["singleSelectClearableUserPickerFields"] =
                json!([{ "fieldId": "assignee", "user": user }]);
        }
        if change_priority.get() && !priority.get().is_empty() {
            actions.push("priority");
            input["priority"] = json!({ "priorityId": priority.get() });
        }
        if change_labels.get() {
            actions.push("labels");
            let names: Vec<Value> = labels
                .get()
                .split_whitespace()
                .map(|n| json!({ "name": n }))
                .collect();
            input["labelsFields"] = json!([{ "fieldId": "labels", "labels": names, "bulkEditMultiSelectFieldOption": label_mode.get() }]);
        }
        if change_due.get() {
            actions.push("duedate");
            let date = if due.get().is_empty() {
                Value::Null
            } else {
                json!({ "formattedDate": due.get() })
            };
            input["datePickerFields"] = json!([{ "fieldId": "duedate", "date": date }]);
        }
        submit.run((
            "issues/fields".into(),
            json!({ "selectedIssueIdsOrKeys": keys.get_value(), "selectedActions": actions, "editedFieldsInput": input }),
        ));
    };
    view! {
        <div class="form bulk-edit">
            <p class="muted small">"Tick the fields to change. The others stay as they are."</p>
            <label class="check-row">
                <input type="checkbox" prop:checked=move || change_assignee.get() on:change:target=move |ev| change_assignee.set(ev.target().checked())/>
                "Assignee"
            </label>
            <Show when=move || change_assignee.get()>
                <UserSelect users=users selected=None on_change=Callback::new(move |id| assignee.set(id))/>
            </Show>
            <label class="check-row">
                <input type="checkbox" prop:checked=move || change_priority.get() on:change:target=move |ev| change_priority.set(ev.target().checked())/>
                "Priority"
            </label>
            <Show when=move || change_priority.get()>
                <select class="select" aria-label="Priority" on:change:target=move |ev| priority.set(ev.target().value())>
                    <option value="">"Pick a priority"</option>
                    {move || priorities.get().unwrap_or_default().into_iter().map(|p| view! { <option value=p.id>{p.name}</option> }).collect_view()}
                </select>
            </Show>
            <label class="check-row">
                <input type="checkbox" prop:checked=move || change_labels.get() on:change:target=move |ev| change_labels.set(ev.target().checked())/>
                "Labels"
            </label>
            <Show when=move || change_labels.get()>
                <div class="inline-form">
                    <select class="select" aria-label="How to change labels" on:change:target=move |ev| label_mode.set(ev.target().value())>
                        <option value="ADD">"Add"</option>
                        <option value="REMOVE">"Remove"</option>
                        <option value="REPLACE">"Replace all with"</option>
                        <option value="REMOVE_ALL">"Remove all"</option>
                    </select>
                    <input class="input" placeholder="Labels, separated by spaces" prop:value=move || labels.get()
                        on:input:target=move |ev| labels.set(ev.target().value())/>
                </div>
            </Show>
            <label class="check-row">
                <input type="checkbox" prop:checked=move || change_due.get() on:change:target=move |ev| change_due.set(ev.target().checked())/>
                "Due date"
            </label>
            <Show when=move || change_due.get()>
                <input class="input" type="date" aria-label="Due date" prop:value=move || due.get()
                    on:input:target=move |ev| due.set(ev.target().value())/>
            </Show>
            <div class="form-actions">
                <button class="button primary" disabled=move || busy.get() on:click=apply>"Apply"</button>
            </div>
        </div>
    }
}

#[component]
fn TransitionForm(
    keys: Vec<String>,
    submit: Callback<(String, Value)>,
    busy: RwSignal<bool>,
) -> impl IntoView {
    let list = keys.join(",");
    let groups = LocalResource::new(move || {
        let list = list.clone();
        async move {
            api::get::<Value>(&format!(
                "{}/bulk/issues/transition?issueIdsOrKeys={}",
                api::CORE,
                api::encode(&list)
            ))
            .await
            .map(|v| {
                v["availableTransitions"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
        }
    });
    // The transition picked for each group, by group position.
    let picked = RwSignal::new(std::collections::HashMap::<usize, String>::new());
    let apply = move |_| {
        let Some(Ok(groups)) = groups.get_untracked() else {
            return;
        };
        let chosen = picked.get_untracked();
        let inputs: Vec<Value> = groups
            .iter()
            .enumerate()
            .filter_map(|(i, group)| {
                let transition = chosen.get(&i).filter(|t| !t.is_empty())?;
                Some(json!({ "selectedIssueIdsOrKeys": group["issues"], "transitionId": transition }))
            })
            .collect();
        if inputs.is_empty() {
            return;
        }
        submit.run((
            "issues/transition".into(),
            json!({ "bulkTransitionInputs": inputs }),
        ));
    };
    view! {
        <div class="form">
            <Transition fallback=|| view! { <p class="muted">"Loading transitions…"</p> }>
                {move || groups.get().map(|result| match result {
                    Ok(groups) => {
                        groups.into_iter().enumerate().map(|(i, group)| {
                            let issues: Vec<String> = group["issues"].as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_string)).collect();
                            view! {
                                <div class="transition-group">
                                    <p class="small">{issues.join(", ")}</p>
                                    <select class="select" aria-label="Transition"
                                        on:change:target=move |ev| { let value = ev.target().value(); picked.update(|p| { p.insert(i, value); }); }>
                                        <option value="">"Leave as it is"</option>
                                        {group["transitions"].as_array().cloned().unwrap_or_default().into_iter().map(|t| {
                                            let id = t["transitionId"].to_string();
                                            let name = t["transitionName"].as_str().unwrap_or_default();
                                            let to = t["to"]["statusName"].as_str().unwrap_or_default();
                                            let label = if name == to { name.to_string() } else { format!("{name} → {to}") };
                                            view! { <option value=id>{label}</option> }
                                        }).collect_view()}
                                    </select>
                                </div>
                            }
                        }).collect_view().into_any()
                    }
                    Err(e) => view! { <p class="error-text">{e.message}</p> }.into_any(),
                })}
            </Transition>
            <div class="form-actions">
                <button class="button primary" disabled=move || busy.get() on:click=apply>"Transition"</button>
            </div>
        </div>
    }
}

#[component]
fn MoveForm(
    keys: Vec<String>,
    submit: Callback<(String, Value)>,
    busy: RwSignal<bool>,
) -> impl IntoView {
    let options = LocalResource::new(|| async {
        let projects = api::get::<Vec<Project>>(&format!("{}/project", api::CORE))
            .await
            .unwrap_or_default();
        let types = api::get::<Vec<Named>>(&format!("{}/issuetype", api::CORE))
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|t| !t.subtask)
            .collect::<Vec<_>>();
        (projects, types)
    });
    let project = RwSignal::new(String::new());
    let issue_type = RwSignal::new(String::new());
    let keys = StoredValue::new(keys);
    let apply = move |_| {
        if project.get().is_empty() || issue_type.get().is_empty() {
            return;
        }
        let target = format!("{},{}", project.get(), issue_type.get());
        submit.run((
            "issues/move".into(),
            json!({ "targetToSourcesMapping": { target: { "issueIdsOrKeys": keys.get_value(), "inferFieldDefaults": true, "inferStatusDefaults": true } } }),
        ));
    };
    view! {
        <div class="form">
            <p class="muted small">"Moved issues get keys in the new project; their old keys still find them. Subtasks move with their parents. Components and versions stay with the old project."</p>
            <label>"Project"
                <select class="select" on:change:target=move |ev| project.set(ev.target().value())>
                    <option value="">"Pick a project"</option>
                    {move || options.get().map(|(projects, _)| projects.into_iter().map(|p| view! { <option value=p.key.clone()>{format!("{} ({})", p.name, p.key)}</option> }).collect_view())}
                </select>
            </label>
            <label>"Issue type"
                <select class="select" on:change:target=move |ev| issue_type.set(ev.target().value())>
                    <option value="">"Pick an issue type"</option>
                    {move || options.get().map(|(_, types)| types.into_iter().map(|t| view! { <option value=t.id>{t.name}</option> }).collect_view())}
                </select>
            </label>
            <div class="form-actions">
                <button class="button primary" disabled=move || busy.get() on:click=apply>"Move"</button>
            </div>
        </div>
    }
}
