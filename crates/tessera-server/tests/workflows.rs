//! Custom statuses, workflows, and workflow schemes.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::{Value, json};

const API: &str = "/rest/api/3";

/// The names of the transitions an issue offers now.
async fn transition_names(app: &TestApp, key: &str) -> Vec<String> {
    let (_, list) = app.get(&format!("{API}/issue/{key}/transitions")).await;
    list["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

async fn transition(app: &TestApp, key: &str, name: &str) {
    let (_, list) = app.get(&format!("{API}/issue/{key}/transitions")).await;
    let id = list["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("no transition {name}"))["id"]
        .clone();
    let (status, body) = app
        .post(
            &format!("{API}/issue/{key}/transitions"),
            json!({ "transition": { "id": id } }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

async fn status_of(app: &TestApp, key: &str) -> String {
    let (_, issue) = app.get(&format!("{API}/issue/{key}?fields=status")).await;
    issue["fields"]["status"]["name"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A review workflow: To Do -> In Progress -> In Review -> Done, and Reopen from anywhere.
async fn review_workflow(app: &TestApp) -> Value {
    let (status, created) = app
        .post(
            &format!("{API}/statuses"),
            json!({ "scope": { "type": "GLOBAL" }, "statuses": [{ "name": "In Review", "statusCategory": "IN_PROGRESS" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let review = created[0]["id"].as_str().unwrap().to_string();
    let (status, workflow) = app
        .post(
            &format!("{API}/workflow"),
            json!({
                "name": "Review Workflow",
                "statuses": [{ "id": "10000" }, { "id": "10001" }, { "id": review }, { "id": "10002" }],
                "transitions": [
                    { "name": "Create", "to": "10000", "type": "initial" },
                    { "name": "Start", "from": ["10000"], "to": "10001", "type": "directed" },
                    { "name": "Send to review", "from": ["10001"], "to": review, "type": "directed" },
                    { "name": "Approve", "from": [review], "to": "10002", "type": "directed" },
                    { "name": "Reopen", "to": "10000", "type": "global" },
                ],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{workflow}");
    workflow
}

#[tokio::test]
async fn projects_follow_their_workflow_scheme() {
    let app = TestApp::new().await;
    app.post(
        &format!("{API}/project"),
        json!({ "key": "REV", "name": "Review", "projectTemplateKey": "com.pyxis.greenhopper.jira:gh-simplified-kanban-classic" }),
    )
    .await;
    let old = app.issue("REV", "Before the change", json!({})).await;
    review_workflow(&app).await;

    let (status, scheme) = app
        .post(
            &format!("{API}/workflowscheme"),
            json!({ "name": "Review Scheme", "defaultWorkflow": "Review Workflow" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{scheme}");
    let (_, project) = app.get(&format!("{API}/project/REV")).await;
    let (status, _) = app
        .put(
            &format!("{API}/workflowscheme/project"),
            json!({ "workflowSchemeId": scheme["id"], "projectId": project["id"] }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // New issues start at To Do and move only along the workflow.
    let key = app.issue("REV", "Needs review", json!({})).await;
    assert_eq!(status_of(&app, &key).await, "To Do");
    assert_eq!(transition_names(&app, &key).await, ["Start"]);
    transition(&app, &key, "Start").await;
    transition(&app, &key, "Send to review").await;
    assert_eq!(status_of(&app, &key).await, "In Review");
    assert_eq!(transition_names(&app, &key).await, ["Approve", "Reopen"]);
    transition(&app, &key, "Approve").await;
    let (_, done) = app
        .get(&format!("{API}/issue/{key}?fields=resolution"))
        .await;
    assert_eq!(
        done["fields"]["resolution"]["name"], "Done",
        "entering Done resolves the issue"
    );
    assert_eq!(status_of(&app, &old).await, "To Do");

    // The board shows the new status.
    let (_, boards) = app.get("/rest/agile/1.0/board?projectKeyOrId=REV").await;
    let board = boards["values"][0]["id"].as_i64().unwrap();
    let (_, config) = app
        .get(&format!("/rest/agile/1.0/board/{board}/configuration"))
        .await;
    let columns: Vec<&str> = config["columnConfig"]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(columns.contains(&"In Review"), "{columns:?}");
    assert_eq!(columns.last(), Some(&"Done"));

    // The project lists the workflow's statuses for each issue type.
    let (_, statuses) = app.get(&format!("{API}/project/REV/statuses")).await;
    assert_eq!(statuses[0]["statuses"].as_array().unwrap().len(), 4);

    // Bugs can keep the simplified workflow.
    let bug = "10003";
    let (status, updated) = app
        .put(
            &format!("{API}/workflowscheme/{}", scheme["id"]),
            json!({ "issueTypeMappings": { bug: "Simplified Workflow" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["issueTypeMappings"][bug], "Simplified Workflow");
}

#[tokio::test]
async fn changing_workflows_moves_issues_and_protects_what_is_in_use() {
    let app = TestApp::new().await;
    app.project("REV").await;
    let workflow = review_workflow(&app).await;
    let (_, scheme) = app
        .post(
            &format!("{API}/workflowscheme"),
            json!({ "name": "Review Scheme", "defaultWorkflow": "Review Workflow" }),
        )
        .await;
    let (_, project) = app.get(&format!("{API}/project/REV")).await;
    app.put(
        &format!("{API}/workflowscheme/project"),
        json!({ "workflowSchemeId": scheme["id"], "projectId": project["id"] }),
    )
    .await;
    let key = app.issue("REV", "In review", json!({})).await;
    transition(&app, &key, "Start").await;
    transition(&app, &key, "Send to review").await;

    // Back to the default scheme: In Review is not in it, so the issue moves to a status of
    // the same category.
    app.put(
        &format!("{API}/workflowscheme/project"),
        json!({ "workflowSchemeId": 10000, "projectId": project["id"] }),
    )
    .await;
    assert_eq!(status_of(&app, &key).await, "In Progress");

    // Workflows and statuses in use cannot be deleted.
    let id = workflow["entityId"].as_str().unwrap();
    let (status, _) = app.delete(&format!("{API}/workflow/{id}")).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the Review Scheme still uses it"
    );
    let (status, _) = app
        .delete(&format!("{API}/workflowscheme/{}", scheme["id"]))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app.delete(&format!("{API}/workflow/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, found) = app
        .get(&format!(
            "{API}/workflow/search?workflowName=Simplified%20Workflow"
        ))
        .await;
    assert_eq!(
        found["values"][0]["transitions"].as_array().unwrap().len(),
        4
    );
    let (_, statuses) = app
        .get(&format!("{API}/statuses/search?searchString=review"))
        .await;
    let review = statuses["values"][0]["id"].as_str().unwrap();
    let (status, _) = app.delete(&format!("{API}/statuses?id={review}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}
