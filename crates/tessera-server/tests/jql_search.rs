//! JQL search, end to end: parse, compile to SQL, run, and render.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::json;

/// Three issues with different statuses, priorities, people, labels, and text.
async fn seeded() -> TestApp {
    let app = TestApp::new().await;
    app.project("WEB").await;
    app.project("API").await;
    let (_, bob) = app.user("bob@tessera.test", "Bob Builder").await;
    app.issue(
        "WEB",
        "Login page crashes",
        json!({ "issuetype": { "name": "Bug" }, "priority": { "name": "Highest" }, "labels": ["frontend", "urgent"],
                "description": "The login form throws on submit", "duedate": "2026-01-15" }),
    )
    .await;
    app.issue(
        "WEB",
        "Polish the dashboard",
        json!({ "issuetype": { "name": "Story" }, "priority": { "name": "Low" }, "assignee": { "accountId": bob },
                "labels": ["frontend"] }),
    )
    .await;
    app.issue(
        "API",
        "Rate limit the search endpoint",
        json!({ "assignee": { "accountId": bob } }),
    )
    .await;
    // Move WEB-1 to Done and add a comment to API-1.
    let (status, _) = app
        .post(
            "/rest/api/3/issue/WEB-1/transitions",
            json!({ "transition": { "id": "31" } }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .post(
            "/rest/api/2/issue/API-1/comment",
            json!({ "body": "Needs a token bucket" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    app
}

fn sorted(mut keys: Vec<String>) -> Vec<String> {
    keys.sort();
    keys
}

#[tokio::test]
async fn equality_lists_and_negation() {
    let app = seeded().await;
    assert_eq!(sorted(app.jql("project = WEB").await), ["WEB-1", "WEB-2"]);
    assert_eq!(app.jql("project = API").await, ["API-1"]);
    assert_eq!(sorted(app.jql("project in (WEB, API)").await).len(), 3);
    assert_eq!(app.jql("issuetype = Bug").await, ["WEB-1"]);
    assert_eq!(app.jql("status = Done").await, ["WEB-1"]);
    assert_eq!(sorted(app.jql("status != Done").await), ["API-1", "WEB-2"]);
    assert_eq!(
        sorted(app.jql(r#"status in ("To Do", "In Progress")"#).await),
        ["API-1", "WEB-2"]
    );
    assert_eq!(app.jql("statusCategory = Done").await, ["WEB-1"]);
    assert_eq!(app.jql("NOT project = WEB").await, ["API-1"]);
    assert_eq!(
        app.jql("project = WEB AND (status = Done OR priority = Low) AND type = Story")
            .await,
        ["WEB-2"]
    );
}

#[tokio::test]
async fn resolution_and_empty() {
    let app = seeded().await;
    assert_eq!(app.jql("resolution = Done").await, ["WEB-1"]);
    assert_eq!(
        sorted(app.jql("resolution = Unresolved").await),
        ["API-1", "WEB-2"]
    );
    assert_eq!(
        sorted(app.jql("resolution IS EMPTY").await),
        ["API-1", "WEB-2"]
    );
    assert_eq!(app.jql("assignee IS EMPTY").await, ["WEB-1"]);
    // != never matches an empty field, as in Jira.
    assert_eq!(app.jql("assignee != admin@tessera.test").await.len(), 2);
}

#[tokio::test]
async fn users_and_current_user() {
    let app = seeded().await;
    assert_eq!(
        sorted(app.jql(r#"assignee = "Bob Builder""#).await),
        ["API-1", "WEB-2"]
    );
    assert_eq!(
        sorted(app.jql("assignee = bob@tessera.test").await),
        ["API-1", "WEB-2"]
    );
    assert_eq!(app.jql("reporter = currentUser()").await.len(), 3);
    assert!(app.jql("assignee = currentUser()").await.is_empty());
}

#[tokio::test]
async fn priority_comparisons() {
    let app = seeded().await;
    // More urgent than Medium.
    assert_eq!(app.jql("priority > Medium").await, ["WEB-1"]);
    assert_eq!(
        sorted(app.jql("priority <= Medium").await),
        ["API-1", "WEB-2"]
    );
}

#[tokio::test]
async fn text_search() {
    let app = seeded().await;
    assert_eq!(app.jql(r#"summary ~ "login""#).await, ["WEB-1"]);
    assert_eq!(
        app.jql(r#"summary ~ "rate search""#).await,
        ["API-1"],
        "all words must match"
    );
    assert_eq!(app.jql(r#"summary ~ "dash*""#).await, ["WEB-2"]);
    assert_eq!(app.jql(r#"description ~ "submit""#).await, ["WEB-1"]);
    assert_eq!(app.jql(r#"comment ~ "bucket""#).await, ["API-1"]);
    assert_eq!(
        sorted(app.jql(r#"text ~ "bucket" OR text ~ "throws""#).await),
        ["API-1", "WEB-1"]
    );
    assert_eq!(
        app.jql(r#"summary !~ "login" AND project = WEB"#).await,
        ["WEB-2"]
    );
}

#[tokio::test]
async fn labels_keys_and_dates() {
    let app = seeded().await;
    assert_eq!(
        sorted(app.jql("labels = frontend").await),
        ["WEB-1", "WEB-2"]
    );
    assert_eq!(app.jql("labels in (urgent)").await, ["WEB-1"]);
    assert_eq!(app.jql("labels is EMPTY").await, ["API-1"]);
    assert_eq!(app.jql("labels != urgent").await, ["WEB-2"]);
    assert_eq!(app.jql("key = WEB-2").await, ["WEB-2"]);
    assert_eq!(app.jql("key > WEB-1").await, ["WEB-2"]);
    assert_eq!(app.jql("issue in (API-1)").await, ["API-1"]);
    assert_eq!(app.jql("created >= -1d").await.len(), 3);
    assert!(app.jql("created < -1d").await.is_empty());
    assert_eq!(
        app.jql("created >= startOfDay() AND created <= endOfDay()")
            .await
            .len(),
        3
    );
    assert_eq!(app.jql(r#"duedate < "2026-02-01""#).await, ["WEB-1"]);
    assert_eq!(app.jql("resolved IS NOT EMPTY").await, ["WEB-1"]);
}

#[tokio::test]
async fn history_searches() {
    let app = seeded().await;
    assert_eq!(
        app.jql(r#"status WAS "To Do""#).await.len(),
        3,
        "includes the current status"
    );
    assert_eq!(app.jql("status CHANGED").await, ["WEB-1"]);
    assert_eq!(
        app.jql(r#"status CHANGED FROM "To Do" TO Done"#).await,
        ["WEB-1"]
    );
    assert_eq!(
        app.jql("status CHANGED BY currentUser() AFTER -1h").await,
        ["WEB-1"]
    );
    assert!(app.jql("status CHANGED BEFORE -1d").await.is_empty());
    assert_eq!(
        sorted(app.jql("status WAS NOT Done").await),
        ["API-1", "WEB-2"]
    );
    assert_eq!(
        app.jql(r#"status WAS IN (Done) BY "admin@tessera.test""#)
            .await,
        ["WEB-1"]
    );
}

#[tokio::test]
async fn ordering() {
    let app = seeded().await;
    assert_eq!(
        app.jql("ORDER BY key ASC").await,
        ["API-1", "WEB-1", "WEB-2"]
    );
    assert_eq!(
        app.jql("project = WEB ORDER BY priority DESC").await,
        ["WEB-1", "WEB-2"]
    );
    assert_eq!(
        app.jql("project = WEB ORDER BY priority ASC").await,
        ["WEB-2", "WEB-1"]
    );
    assert_eq!(
        app.jql("ORDER BY summary").await,
        ["WEB-1", "WEB-2", "API-1"]
    );
    // Default: newest first.
    assert_eq!(app.jql("").await, ["API-1", "WEB-2", "WEB-1"]);
}

#[tokio::test]
async fn errors_use_jira_wording() {
    let app = &seeded().await;
    let error = |jql: &'static str| async move {
        let (status, body) = app.post("/rest/api/3/search", json!({ "jql": jql })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{jql}");
        body["errorMessages"][0].as_str().unwrap().to_string()
    };
    assert_eq!(
        error("nosuchfield = x").await,
        "Field 'nosuchfield' does not exist or you do not have permission to view it."
    );
    assert_eq!(
        error("status = Nope").await,
        "The value 'Nope' does not exist for the field 'status'."
    );
    assert_eq!(
        error("status ~ Done").await,
        "The operator '~' is not supported by the 'status' field."
    );
    assert!(
        error("project = ")
            .await
            .starts_with("Error in the JQL Query:")
    );
    assert!(error("created > tomorrowish").await.contains("is invalid"));
}

#[tokio::test]
async fn paging_in_both_search_apis() {
    let app = seeded().await;
    let (_, page) = app
        .post(
            "/rest/api/3/search",
            json!({ "jql": "ORDER BY key", "startAt": 1, "maxResults": 1 }),
        )
        .await;
    assert_eq!(page["total"], 3);
    assert_eq!(page["issues"][0]["key"], "WEB-1");

    // The newer API returns only ids unless fields are requested, and pages by token.
    let (_, first) = app
        .post(
            "/rest/api/3/search/jql",
            json!({ "jql": "ORDER BY key", "maxResults": 2 }),
        )
        .await;
    assert_eq!(first["issues"].as_array().unwrap().len(), 2);
    assert!(first["issues"][0].get("key").is_none());
    assert_eq!(first["isLast"], false);
    let token = first["nextPageToken"].as_str().unwrap();
    let (_, second) = app
        .post(
            "/rest/api/3/search/jql",
            json!({ "jql": "ORDER BY key", "maxResults": 2, "nextPageToken": token, "fields": ["summary"] }),
        )
        .await;
    assert_eq!(second["issues"][0]["key"], "WEB-2");
    assert_eq!(second["isLast"], true);
    assert!(second.get("nextPageToken").is_none());

    let (status, legacy) = app
        .get("/rest/api/2/search?jql=project%20%3D%20WEB&fields=summary")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(legacy["total"], 2);
}

#[tokio::test]
async fn custom_field_search() {
    let app = seeded().await;
    let (_, points) = app
        .post(
            "/rest/api/3/field",
            json!({ "name": "Story Points", "type": "com.atlassian.jira.plugin.system.customfieldtypes:float" }),
        )
        .await;
    let key = points["id"].as_str().unwrap().to_string();
    let id = points["schema"]["customId"].as_i64().unwrap();
    app.put(
        "/rest/api/3/issue/WEB-2",
        json!({ "fields": { key.clone(): 8 } }),
    )
    .await;
    app.put(
        "/rest/api/3/issue/API-1",
        json!({ "fields": { key.clone(): 3 } }),
    )
    .await;
    assert_eq!(app.jql(&format!("cf[{id}] > 5")).await, ["WEB-2"]);
    assert_eq!(app.jql(r#""Story Points" = 3"#).await, ["API-1"]);
    assert_eq!(app.jql(r#""Story Points" is EMPTY"#).await, ["WEB-1"]);
    assert_eq!(
        app.jql(&format!("project = WEB ORDER BY cf[{id}] DESC"))
            .await,
        ["WEB-2", "WEB-1"]
    );
}
