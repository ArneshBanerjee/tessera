//! Saved filters and the Jira Software API: boards, sprints, backlog, ranking, and epics.

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::{Value, json};

/// A scrum project (with its board) and three tasks.
async fn scrum() -> (TestApp, i64) {
    let app = TestApp::new().await;
    let (status, _) = app
        .post(
            "/rest/api/3/project",
            json!({ "key": "SCRUM", "name": "Scrum", "projectTemplateKey": "com.pyxis.greenhopper.jira:gh-simplified-scrum-classic" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    for summary in ["First", "Second", "Third"] {
        app.issue("SCRUM", summary, json!({})).await;
    }
    let (_, boards) = app.get("/rest/agile/1.0/board?projectKeyOrId=SCRUM").await;
    let board = boards["values"][0]["id"]
        .as_i64()
        .expect("the template made a board");
    (app, board)
}

fn keys(page: &Value) -> Vec<String> {
    page["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn saved_filters() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    app.issue("PROJ", "Bug one", json!({ "issuetype": { "name": "Bug" } }))
        .await;
    app.issue("PROJ", "Task one", json!({})).await;

    let (status, err) = app
        .post(
            "/rest/api/3/filter",
            json!({ "name": "Broken", "jql": "status = Nope" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["errors"]["jql"]
            .as_str()
            .unwrap()
            .contains("does not exist"),
        "{err}"
    );

    let (status, filter) = app
        .post(
            "/rest/api/3/filter",
            json!({ "name": "My bugs", "jql": "type = Bug", "favourite": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{filter}");
    let id = filter["id"].as_str().unwrap().to_string();
    assert_eq!(filter["favourite"], true);
    assert_eq!(filter["owner"]["displayName"], "Ada Admin");

    let (status, _) = app
        .post(
            "/rest/api/3/filter",
            json!({ "name": "my BUGS", "jql": "" }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "names are unique per owner"
    );

    // JQL can use the saved filter by id or name.
    assert_eq!(app.jql(&format!("filter = {id}")).await, ["PROJ-1"]);
    assert_eq!(
        app.jql(r#"filter in ("My bugs") OR summary ~ task"#)
            .await
            .len(),
        2
    );
    assert_eq!(app.jql(&format!("filter != {id}")).await, ["PROJ-2"]);

    let (_, favourites) = app.get("/rest/api/3/filter/favourite").await;
    assert_eq!(favourites[0]["name"], "My bugs");
    let (_, search) = app.get("/rest/api/3/filter/search?filterName=bug").await;
    assert_eq!(search["total"], 1);

    // Private filters are invisible to others until shared.
    let (bob, _) = app.user("bob@tessera.test", "Bob").await;
    let path = format!("/rest/api/3/filter/{id}");
    let (status, _) = app.request(Method::GET, &path, Some(&bob), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    app.put(
        &path,
        json!({ "sharePermissions": [{ "type": "loggedin" }] }),
    )
    .await;
    let (status, shared) = app.request(Method::GET, &path, Some(&bob), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shared["isWritable"], false);
    let (status, _) = app
        .request(
            Method::PUT,
            &path,
            Some(&bob),
            Some(json!({ "name": "Mine now" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "only the owner edits");

    let (status, _) = app.delete(&path).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn project_templates_create_boards() {
    let (app, board) = scrum().await;
    let (_, info) = app.get(&format!("/rest/agile/1.0/board/{board}")).await;
    assert_eq!(info["name"], "SCRUM board");
    assert_eq!(info["type"], "scrum");
    assert_eq!(info["location"]["projectKey"], "SCRUM");

    let (_, config) = app
        .get(&format!("/rest/agile/1.0/board/{board}/configuration"))
        .await;
    let columns: Vec<&str> = config["columnConfig"]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(columns, ["To Do", "In Progress", "Done"]);
    assert_eq!(
        config["estimation"]["field"]["displayName"],
        "Story point estimate"
    );

    // The board's filter cannot be deleted while the board uses it.
    let filter_id = config["filter"]["id"].as_str().unwrap();
    let (status, _) = app.delete(&format!("/rest/api/3/filter/{filter_id}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, issues) = app
        .get(&format!("/rest/agile/1.0/board/{board}/issue"))
        .await;
    assert_eq!(
        keys(&issues),
        ["SCRUM-1", "SCRUM-2", "SCRUM-3"],
        "rank order is creation order"
    );
    assert!(issues["issues"][0]["fields"]["sprint"].is_null());

    // Business projects get no board.
    let (_, _) = app
        .post("/rest/api/3/project", json!({ "key": "BIZ", "name": "Biz", "projectTypeKey": "business", "projectTemplateKey": "kanban" }))
        .await;
    let (_, none) = app.get("/rest/agile/1.0/board?projectKeyOrId=BIZ").await;
    assert_eq!(none["total"], 0);
}

#[tokio::test]
async fn sprint_lifecycle() {
    let (app, board) = scrum().await;
    let (status, sprint) = app
        .post(
            "/rest/agile/1.0/sprint",
            json!({ "name": "Sprint 1", "originBoardId": board, "goal": "Ship it" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sprint}");
    assert_eq!(sprint["state"], "future");
    let id = sprint["id"].as_i64().unwrap();

    let (status, _) = app
        .post(
            &format!("/rest/agile/1.0/sprint/{id}/issue"),
            json!({ "issues": ["SCRUM-1", "SCRUM-2"] }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // openSprints() means started and not completed, so a future sprint is not in it.
    assert!(app.jql("sprint in openSprints()").await.is_empty());
    assert_eq!(app.jql("sprint in futureSprints()").await.len(), 2);
    assert_eq!(app.jql(r#"sprint = "Sprint 1""#).await.len(), 2);

    let (_, backlog) = app
        .get(&format!("/rest/agile/1.0/board/{board}/backlog"))
        .await;
    assert_eq!(keys(&backlog), ["SCRUM-3"]);

    // Starting needs dates.
    let path = format!("/rest/agile/1.0/sprint/{id}");
    let (status, _) = app.post(&path, json!({ "state": "active" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, started) = app
        .post(&path, json!({ "state": "active", "startDate": "2026-10-13T09:00:00.000+0000", "endDate": "2026-10-27T09:00:00.000+0000" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(started["state"], "active");
    assert_eq!(app.jql("sprint in openSprints()").await.len(), 2);

    // Only one active sprint per board.
    let (_, second) = app
        .post(
            "/rest/agile/1.0/sprint",
            json!({ "name": "Sprint 2", "originBoardId": board }),
        )
        .await;
    let second_id = second["id"].as_i64().unwrap();
    let (status, _) = app
        .post(
            &format!("/rest/agile/1.0/sprint/{second_id}"),
            json!({ "state": "active", "startDate": "2026-10-13T09:00:00.000+0000", "endDate": "2026-10-27T09:00:00.000+0000" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Finish SCRUM-1, then complete the sprint: SCRUM-2 returns to the backlog.
    app.post(
        "/rest/api/3/issue/SCRUM-1/transitions",
        json!({ "transition": { "id": "31" } }),
    )
    .await;
    let (_, closed) = app.post(&path, json!({ "state": "closed" })).await;
    assert_eq!(closed["state"], "closed");
    assert!(closed["completeDate"].is_string());
    let (_, backlog) = app
        .get(&format!("/rest/agile/1.0/board/{board}/backlog"))
        .await;
    assert_eq!(keys(&backlog), ["SCRUM-2", "SCRUM-3"]);
    assert_eq!(app.jql("sprint in closedSprints()").await.len(), 2);

    let (_, agile_issue) = app.get("/rest/agile/1.0/issue/SCRUM-2").await;
    assert!(agile_issue["fields"]["sprint"].is_null());
    assert_eq!(
        agile_issue["fields"]["closedSprints"][0]["name"],
        "Sprint 1"
    );

    // Closed sprints only change name and goal, and cannot be deleted.
    let (_, renamed) = app
        .post(&path, json!({ "name": "Sprint One", "state": "future" }))
        .await;
    assert_eq!(renamed["name"], "Sprint One");
    assert_eq!(renamed["state"], "closed");
    let (status, _) = app.delete(&path).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The sprint change is in the issue history.
    let (_, history) = app.get("/rest/api/3/issue/SCRUM-2/changelog").await;
    assert!(history.to_string().contains("\"field\":\"Sprint\""));

    // Deleting a future sprint returns its issues to the backlog.
    app.post(
        &format!("/rest/agile/1.0/sprint/{second_id}/issue"),
        json!({ "issues": ["SCRUM-3"] }),
    )
    .await;
    let (status, _) = app
        .delete(&format!("/rest/agile/1.0/sprint/{second_id}"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, backlog) = app
        .get(&format!("/rest/agile/1.0/board/{board}/backlog"))
        .await;
    assert!(keys(&backlog).contains(&"SCRUM-3".to_string()));

    let (_, sprints) = app
        .get(&format!(
            "/rest/agile/1.0/board/{board}/sprint?state=closed"
        ))
        .await;
    assert_eq!(sprints["total"], 1);
}

#[tokio::test]
async fn sprint_field_and_backlog_api() {
    let (app, board) = scrum().await;
    let (_, sprint) = app
        .post(
            "/rest/agile/1.0/sprint",
            json!({ "name": "S1", "originBoardId": board }),
        )
        .await;
    let id = sprint["id"].as_i64().unwrap();
    let (_, fields) = app.get("/rest/api/3/field").await;
    let sprint_field = fields
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "Sprint")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // The Sprint field moves an issue; its value lists the sprints.
    let (status, _) = app
        .put(
            "/rest/api/3/issue/SCRUM-3",
            json!({ "fields": { sprint_field.clone(): id } }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, issue) = app.get("/rest/api/3/issue/SCRUM-3").await;
    assert_eq!(issue["fields"][&sprint_field][0]["name"], "S1");

    let (status, _) = app
        .post(
            "/rest/agile/1.0/backlog/issue",
            json!({ "issues": ["SCRUM-3"] }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, issue) = app.get("/rest/api/3/issue/SCRUM-3").await;
    assert!(issue["fields"][&sprint_field].is_null());
}

#[tokio::test]
async fn ranking() {
    let (app, board) = scrum().await;
    let (status, _) = app
        .put(
            "/rest/agile/1.0/issue/rank",
            json!({ "issues": ["SCRUM-3"], "rankBeforeIssue": "SCRUM-1" }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, issues) = app
        .get(&format!("/rest/agile/1.0/board/{board}/issue"))
        .await;
    assert_eq!(keys(&issues), ["SCRUM-3", "SCRUM-1", "SCRUM-2"]);

    app.put(
        "/rest/agile/1.0/issue/rank",
        json!({ "issues": ["SCRUM-3"], "rankAfterIssue": "SCRUM-2" }),
    )
    .await;
    assert_eq!(
        app.jql("project = SCRUM ORDER BY Rank ASC").await,
        ["SCRUM-1", "SCRUM-2", "SCRUM-3"]
    );

    // Ranking between two neighbors many times keeps a strict order.
    for _ in 0..20 {
        app.put(
            "/rest/agile/1.0/issue/rank",
            json!({ "issues": ["SCRUM-3"], "rankBeforeIssue": "SCRUM-2" }),
        )
        .await;
        app.put(
            "/rest/agile/1.0/issue/rank",
            json!({ "issues": ["SCRUM-2"], "rankBeforeIssue": "SCRUM-3" }),
        )
        .await;
    }
    assert_eq!(
        app.jql("project = SCRUM ORDER BY Rank ASC").await,
        ["SCRUM-1", "SCRUM-2", "SCRUM-3"]
    );

    let (status, _) = app
        .put(
            "/rest/agile/1.0/issue/rank",
            json!({ "issues": ["SCRUM-1"], "rankBeforeIssue": "SCRUM-1" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn epics_and_estimation() {
    let (app, board) = scrum().await;
    let epic = app
        .issue(
            "SCRUM",
            "Big feature",
            json!({ "issuetype": { "name": "Epic" } }),
        )
        .await;

    let (status, _) = app
        .post(
            &format!("/rest/agile/1.0/epic/{epic}/issue"),
            json!({ "issues": ["SCRUM-1", "SCRUM-2"] }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, in_epic) = app.get(&format!("/rest/agile/1.0/epic/{epic}/issue")).await;
    assert_eq!(keys(&in_epic), ["SCRUM-1", "SCRUM-2"]);
    let (_, outside) = app
        .get(&format!("/rest/agile/1.0/board/{board}/epic/none/issue"))
        .await;
    assert_eq!(keys(&outside), ["SCRUM-3"]);
    assert_eq!(app.jql(&format!(r#""Epic Link" = {epic}"#)).await.len(), 2);

    let (_, updated) = app
        .post(
            &format!("/rest/agile/1.0/epic/{epic}"),
            json!({ "name": "Feature X", "color": { "key": "color_5" }, "done": true }),
        )
        .await;
    assert_eq!(updated["name"], "Feature X");
    assert_eq!(updated["color"]["key"], "color_5");
    assert_eq!(updated["done"], true);
    let (_, epics) = app
        .get(&format!("/rest/agile/1.0/board/{board}/epic?done=true"))
        .await;
    assert_eq!(epics["values"][0]["key"], epic.as_str());

    let (_, agile_issue) = app.get("/rest/agile/1.0/issue/SCRUM-1").await;
    assert_eq!(agile_issue["fields"]["epic"]["name"], "Feature X");

    let (status, _) = app
        .post(
            "/rest/agile/1.0/epic/none/issue",
            json!({ "issues": ["SCRUM-2"] }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, in_epic) = app.get(&format!("/rest/agile/1.0/epic/{epic}/issue")).await;
    assert_eq!(keys(&in_epic), ["SCRUM-1"]);

    let (_, estimate) = app
        .put(
            &format!("/rest/agile/1.0/issue/SCRUM-1/estimation?boardId={board}"),
            json!({ "value": "8" }),
        )
        .await;
    assert_eq!(estimate["value"], "8");
    let (_, read) = app
        .get(&format!(
            "/rest/agile/1.0/issue/SCRUM-1/estimation?boardId={board}"
        ))
        .await;
    assert_eq!(read["value"], "8");
}

#[tokio::test]
async fn board_properties_and_swap() {
    let (app, board) = scrum().await;
    let path = format!("/rest/agile/1.0/board/{board}/properties/team");
    let (status, _) = app.put(&path, json!({ "name": "Rockets" })).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, property) = app.get(&path).await;
    assert_eq!(property["value"]["name"], "Rockets");
    let (_, keys) = app
        .get(&format!("/rest/agile/1.0/board/{board}/properties"))
        .await;
    assert_eq!(keys["keys"][0]["key"], "team");

    let (_, a) = app
        .post(
            "/rest/agile/1.0/sprint",
            json!({ "name": "A", "originBoardId": board }),
        )
        .await;
    let (_, b) = app
        .post(
            "/rest/agile/1.0/sprint",
            json!({ "name": "B", "originBoardId": board }),
        )
        .await;
    let (status, _) = app
        .post(
            &format!("/rest/agile/1.0/sprint/{}/swap", a["id"]),
            json!({ "sprintToSwapWith": b["id"] }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, sprints) = app
        .get(&format!("/rest/agile/1.0/board/{board}/sprint"))
        .await;
    assert_eq!(sprints["values"][0]["name"], "B");
}

#[tokio::test]
async fn quick_filters() {
    let (app, board) = scrum().await;
    let path = format!("/rest/agile/1.0/board/{board}/quickfilter");
    let (_, list) = app.get(&path).await;
    let names: Vec<&str> = list["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Only My Issues", "Recently Updated"]);

    let manage = format!("/rest/tessera/1.0/board/{board}/quickfilter");
    let (status, _) = app
        .post(&manage, json!({ "name": "Broken", "jql": "status = = x" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "the JQL must parse");
    let (status, created) = app
        .post(
            &manage,
            json!({ "name": "First only", "jql": "summary ~ First" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["position"], 2);

    // Boards apply a quick filter's JQL through the jql parameter.
    let jql = form_urlencoded::byte_serialize(created["jql"].as_str().unwrap().as_bytes())
        .collect::<String>();
    let (_, issues) = app
        .get(&format!("/rest/agile/1.0/board/{board}/issue?jql={jql}"))
        .await;
    assert_eq!(keys(&issues), ["SCRUM-1"]);

    let id = created["id"].as_i64().unwrap();
    let (status, _) = app.delete(&format!("{manage}/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = app.get(&path).await;
    assert_eq!(list["total"], 2);
}
