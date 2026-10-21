//! Bulk operations: create, edit, transition, watch, move, and delete many issues at once.

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::{Value, json};

const API: &str = "/rest/api/3";

/// Reads a bulk task's result.
async fn task(app: &TestApp, created: &Value) -> Value {
    let id = created["taskId"].as_str().expect("a task id");
    let (status, progress) = app.get(&format!("{API}/bulk/queue/{id}")).await;
    assert_eq!(status, StatusCode::OK, "{progress}");
    assert_eq!(progress["status"], "COMPLETE");
    progress
}

#[tokio::test]
async fn bulk_create_reports_each_issue() {
    let app = TestApp::new().await;
    app.project("BULK").await;
    let issue = |summary: &str| json!({ "fields": { "project": { "key": "BULK" }, "issuetype": { "name": "Task" }, "summary": summary } });
    let (status, created) = app
        .post(
            &format!("{API}/issue/bulk"),
            json!({ "issueUpdates": [issue("One"), { "fields": { "project": { "key": "BULK" } } }, issue("Two")] }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["issues"].as_array().unwrap().len(), 2);
    assert_eq!(created["issues"][0]["key"], "BULK-1");
    assert_eq!(created["errors"][0]["failedElementNumber"], 1);
    assert!(created["errors"][0]["elementErrors"]["errors"]["issuetype"].is_string());
}

#[tokio::test]
async fn bulk_edit_transition_watch_and_delete() {
    let app = TestApp::new().await;
    app.project("BULK").await;
    let (_dev, dev_account) = app.user("dev@tessera.test", "Dev").await;
    let mut keys = Vec::new();
    for i in 0..3 {
        keys.push(
            app.issue("BULK", &format!("Item {i}"), json!({ "labels": ["old"] }))
                .await,
        );
    }

    // Edit: priority, assignee, and an added label, on all three, plus one missing issue.
    let mut selected = keys.clone();
    selected.push("BULK-999".into());
    let (status, created) = app
        .post(
            &format!("{API}/bulk/issues/fields"),
            json!({
                "selectedIssueIdsOrKeys": selected,
                "selectedActions": ["priority", "assignee", "labels"],
                "editedFieldsInput": {
                    "priority": { "priorityId": "1" },
                    "singleSelectClearableUserPickerFields": [{ "fieldId": "assignee", "user": { "accountId": dev_account } }],
                    "labelsFields": [{ "fieldId": "labels", "labels": [{ "name": "triaged" }], "bulkEditMultiSelectFieldOption": "ADD" }],
                },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let progress = task(&app, &created).await;
    assert_eq!(progress["totalIssueCount"], 4);
    assert_eq!(
        progress["processedAccessibleIssues"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(progress["invalidOrInaccessibleIssueCount"], 1);
    let (_, issue) = app
        .get(&format!(
            "{API}/issue/{}?fields=priority,assignee,labels",
            keys[0]
        ))
        .await;
    assert_eq!(issue["fields"]["priority"]["id"], "1");
    assert_eq!(issue["fields"]["assignee"]["accountId"], json!(dev_account));
    assert_eq!(issue["fields"]["labels"], json!(["old", "triaged"]));

    // Replacing labels.
    let (_, created) = app
        .post(
            &format!("{API}/bulk/issues/fields"),
            json!({
                "selectedIssueIdsOrKeys": [keys[0]],
                "selectedActions": ["labels"],
                "editedFieldsInput": { "labelsFields": [{ "fieldId": "labels", "labels": [{ "name": "fresh" }], "bulkEditMultiSelectFieldOption": "REPLACE" }] },
            }),
        )
        .await;
    task(&app, &created).await;
    let (_, issue) = app
        .get(&format!("{API}/issue/{}?fields=labels", keys[0]))
        .await;
    assert_eq!(issue["fields"]["labels"], json!(["fresh"]));

    // Transition: the shared transitions, then Done for two issues.
    let (_, available) = app
        .get(&format!(
            "{API}/bulk/issues/transition?issueIdsOrKeys={},{}",
            keys[0], keys[1]
        ))
        .await;
    let group = &available["availableTransitions"][0];
    assert_eq!(group["issues"].as_array().unwrap().len(), 2);
    let done = group["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["to"]["statusName"] == "Done")
        .unwrap()["transitionId"]
        .to_string();
    let (status, created) = app
        .post(
            &format!("{API}/bulk/issues/transition"),
            json!({ "bulkTransitionInputs": [{ "selectedIssueIdsOrKeys": [keys[0], keys[1]], "transitionId": done }] }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        task(&app, &created).await["processedAccessibleIssues"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        app.jql("project = BULK AND statusCategory = Done ORDER BY key")
            .await,
        keys[..2]
    );

    // Watch and unwatch.
    let (_, created) = app
        .post(
            &format!("{API}/bulk/issues/unwatch"),
            json!({ "selectedIssueIdsOrKeys": keys }),
        )
        .await;
    task(&app, &created).await;
    assert!(app.jql("watcher = currentUser()").await.is_empty());
    let (_, created) = app
        .post(
            &format!("{API}/bulk/issues/watch"),
            json!({ "selectedIssueIdsOrKeys": [keys[2]] }),
        )
        .await;
    task(&app, &created).await;
    assert_eq!(app.jql("watcher = currentUser()").await, [keys[2].clone()]);

    // Delete.
    let (_, created) = app
        .post(
            &format!("{API}/bulk/issues/delete"),
            json!({ "selectedIssueIdsOrKeys": keys }),
        )
        .await;
    task(&app, &created).await;
    assert!(app.jql("project = BULK").await.is_empty());
}

#[tokio::test]
async fn bulk_needs_the_same_permissions_as_one_issue() {
    let app = TestApp::new().await;
    app.project("BULK").await;
    let (dev, _) = app.user("dev@tessera.test", "Dev").await;
    let key = app.issue("BULK", "Protected", json!({})).await;
    // Developers cannot delete issues under the default permission scheme.
    let (status, created) = app
        .request(
            Method::POST,
            &format!("{API}/bulk/issues/delete"),
            Some(&dev),
            Some(json!({ "selectedIssueIdsOrKeys": [key] })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["taskId"].as_str().unwrap();
    let (_, progress) = app
        .request(
            Method::GET,
            &format!("{API}/bulk/queue/{id}"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(progress["processedAccessibleIssues"], json!([]));
    assert_eq!(
        progress["failedAccessibleIssues"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(app.jql("project = BULK").await, [key]);
    // Other people cannot read someone's task.
    let (status, _) = app
        .request(
            Method::GET,
            &format!("{API}/bulk/queue/{id}"),
            Some(&common::basic("nobody@tessera.test", "pw")),
            None,
        )
        .await;
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
async fn bulk_move_gives_new_keys_and_keeps_old_ones() {
    let app = TestApp::new().await;
    app.project("OLD").await;
    app.project("NEW").await;
    let (_, component) = app
        .post(
            &format!("{API}/component"),
            json!({ "name": "Backend", "project": "OLD" }),
        )
        .await;
    let parent = app
        .issue(
            "OLD",
            "Parent",
            json!({ "components": [{ "id": component["id"] }] }),
        )
        .await;
    let child = app
        .issue(
            "OLD",
            "Child",
            json!({ "issuetype": { "name": "Subtask" }, "parent": { "key": parent } }),
        )
        .await;
    let (_, types) = app.get(&format!("{API}/issuetype")).await;
    let story = types
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Story")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, created) = app
        .post(
            &format!("{API}/bulk/issues/move"),
            json!({ "targetToSourcesMapping": { format!("NEW,{story}"): { "issueIdsOrKeys": [parent], "inferFieldDefaults": true, "inferStatusDefaults": true } } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let progress = task(&app, &created).await;
    assert_eq!(
        progress["processedAccessibleIssues"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{progress}"
    );

    // The old key still finds the moved issue, through the API and JQL.
    let (status, moved) = app
        .get(&format!(
            "{API}/issue/{parent}?fields=project,issuetype,components"
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(moved["key"], "NEW-1");
    assert_eq!(moved["fields"]["project"]["key"], "NEW");
    assert_eq!(moved["fields"]["issuetype"]["name"], "Story");
    assert_eq!(moved["fields"]["components"], json!([]));
    assert_eq!(app.jql(&format!("key = {parent}")).await, ["NEW-1"]);

    // The subtask moved with its parent.
    let (_, sub) = app
        .get(&format!("{API}/issue/{child}?fields=project,parent"))
        .await;
    assert_eq!(sub["fields"]["project"]["key"], "NEW");
    assert_eq!(sub["key"], "NEW-2");

    // The history records the move.
    let (_, history) = app.get(&format!("{API}/issue/NEW-1/changelog")).await;
    let fields: Vec<&str> = history["values"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|e| e["items"].as_array().unwrap().iter())
        .map(|i| i["field"].as_str().unwrap())
        .collect();
    assert!(
        fields.contains(&"project") && fields.contains(&"Key"),
        "{fields:?}"
    );
}
