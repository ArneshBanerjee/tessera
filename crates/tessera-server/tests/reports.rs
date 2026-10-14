//! Sprint report, velocity, and burndown, rebuilt from issue history.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::{Value, json};

const AGILE: &str = "/rest/agile/1.0";

/// Events must land on distinct milliseconds for the history to order them.
async fn tick() {
    tokio::time::sleep(Duration::from_millis(5)).await;
}

fn keys(list: &Value) -> Vec<String> {
    let mut keys: Vec<String> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap().to_string())
        .collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn reports_follow_what_happened_in_the_sprint() {
    let app = TestApp::new().await;
    app.post(
        "/rest/api/3/project",
        json!({ "key": "AG", "name": "Agile", "projectTemplateKey": "com.pyxis.greenhopper.jira:gh-simplified-scrum-classic" }),
    )
    .await;
    let (_, boards) = app.get(&format!("{AGILE}/board?projectKeyOrId=AG")).await;
    let board = boards["values"][0]["id"].as_i64().unwrap();
    let (_, fields) = app.get("/rest/api/3/field").await;
    let points = fields
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "Story point estimate")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let issue = |summary: &str, estimate: f64| {
        let points = points.clone();
        let summary = summary.to_string();
        let app = &app;
        async move { app.issue("AG", &summary, json!({ points: estimate })).await }
    };
    let done_one = issue("Done in time", 3.0).await;
    let removed = issue("Taken out", 5.0).await;
    let late = issue("Added late", 2.0).await;

    let (_, sprint) = app
        .post(
            &format!("{AGILE}/sprint"),
            json!({ "name": "S1", "originBoardId": board }),
        )
        .await;
    let sprint = sprint["id"].as_i64().unwrap();
    app.post(
        &format!("{AGILE}/sprint/{sprint}/issue"),
        json!({ "issues": [done_one, removed] }),
    )
    .await;
    tick().await;
    let (status, started) = app
        .post(
            &format!("{AGILE}/sprint/{sprint}"),
            json!({ "state": "active", "startDate": "2026-01-01T00:00:00.000+0000", "endDate": "2030-01-15T00:00:00.000+0000" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert!(started["activatedDate"].is_string());
    tick().await;

    // During the sprint: one issue is done, one is taken out, one is added.
    let (_, transitions) = app
        .get(&format!("/rest/api/3/issue/{done_one}/transitions"))
        .await;
    let to_done = transitions["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["to"]["name"] == "Done")
        .unwrap()["id"]
        .clone();
    app.post(
        &format!("/rest/api/3/issue/{done_one}/transitions"),
        json!({ "transition": { "id": to_done } }),
    )
    .await;
    tick().await;
    app.post(
        &format!("{AGILE}/backlog/issue"),
        json!({ "issues": [removed] }),
    )
    .await;
    tick().await;
    app.post(
        &format!("{AGILE}/sprint/{sprint}/issue"),
        json!({ "issues": [late] }),
    )
    .await;
    tick().await;
    app.post(
        &format!("{AGILE}/sprint/{sprint}"),
        json!({ "state": "closed" }),
    )
    .await;

    let (status, report) = app
        .get(&format!(
            "/rest/greenhopper/1.0/rapid/charts/sprintreport?rapidViewId={board}&sprintId={sprint}"
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    let contents = &report["contents"];
    assert_eq!(keys(&contents["completedIssues"]), [done_one.clone()]);
    assert_eq!(
        keys(&contents["issuesNotCompletedInCurrentSprint"]),
        [late.clone()]
    );
    assert_eq!(keys(&contents["puntedIssues"]), [removed.clone()]);
    assert_eq!(contents["issueKeysAddedDuringSprint"][&late], true);
    assert_eq!(contents["completedIssuesEstimateSum"]["value"], 3.0);
    assert_eq!(contents["puntedIssuesEstimateSum"]["value"], 5.0);
    assert_eq!(report["sprint"]["state"], "CLOSED");

    let (_, velocity) = app
        .get(&format!(
            "/rest/greenhopper/1.0/rapid/charts/velocity?rapidViewId={board}"
        ))
        .await;
    let entry = &velocity["velocityStatEntries"][sprint.to_string()];
    assert_eq!(
        entry["estimated"]["value"], 8.0,
        "committed at the start: 3 + 5"
    );
    assert_eq!(entry["completed"]["value"], 3.0);

    // The burndown starts at 8, drops to 5 (done), to 0 (taken out), rises to 2 (added).
    let (_, burndown) = app
        .get(&format!(
            "/rest/tessera/1.0/reports/burndown?sprintId={sprint}"
        ))
        .await;
    let values: Vec<f64> = burndown["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["remaining"].as_f64().unwrap())
        .collect();
    assert_eq!(values, [8.0, 5.0, 0.0, 2.0, 2.0]);
}
