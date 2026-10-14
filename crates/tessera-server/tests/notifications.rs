//! Email notifications: who gets mail about an issue, and what it says.

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::json;

const API: &str = "/rest/api/3";

/// Every queued mail as (recipient, subject, body), oldest first.
async fn outbox(app: &TestApp) -> Vec<(String, String, String)> {
    sqlx::query_as("SELECT to_email, subject, body FROM outbox ORDER BY id")
        .fetch_all(&app.state.db)
        .await
        .unwrap()
}

async fn clear(app: &TestApp) {
    sqlx::query("DELETE FROM outbox")
        .execute(&app.state.db)
        .await
        .unwrap();
}

#[tokio::test]
async fn people_on_the_issue_hear_about_changes_but_not_their_own() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let (dev, account) = app.user("dev@tessera.test", "Dev").await;
    let key = app
        .issue(
            "PROJ",
            "Fix the login",
            json!({ "assignee": { "accountId": account } }),
        )
        .await;

    // The assignee hears about the new issue; the admin made it, so they do not.
    let mail = outbox(&app).await;
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].0, "dev@tessera.test");
    assert_eq!(mail[0].1, format!("[Tessera] ({key}) Fix the login"));
    assert!(
        mail[0]
            .2
            .starts_with(&format!("Ada Admin created {key}: Fix the login"))
    );
    assert!(mail[0].2.contains(&format!("/browse/{key}")));
    clear(&app).await;

    // The assignee comments: the reporter, who also watches, hears it once.
    let (status, _) = app
        .request(
            Method::POST,
            &format!("{API}/issue/{key}/comment"),
            Some(&dev),
            Some(json!({ "body": { "type": "doc", "version": 1, "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "On it." }] }] } })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let mail = outbox(&app).await;
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].0, "admin@tessera.test");
    assert!(mail[0].2.contains("Dev commented on"));
    assert!(mail[0].2.contains("On it."));
    clear(&app).await;

    // A transition by the admin reaches the assignee, who now also watches.
    let (_, transitions) = app.get(&format!("{API}/issue/{key}/transitions")).await;
    let id = transitions["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["to"]["name"] == "In Progress")
        .expect("a transition to In Progress")["id"]
        .as_str()
        .unwrap()
        .to_string();
    app.post(
        &format!("{API}/issue/{key}/transitions"),
        json!({ "transition": { "id": id } }),
    )
    .await;
    let mail = outbox(&app).await;
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].0, "dev@tessera.test");
    assert!(mail[0].2.contains("New status: In Progress"));
    clear(&app).await;

    // Edits list the fields that changed.
    app.put(
        &format!("{API}/issue/{key}"),
        json!({ "fields": { "summary": "Fix the login page" } }),
    )
    .await;
    let mail = outbox(&app).await;
    assert!(mail[0].2.contains("Changed: summary"), "{}", mail[0].2);
}

#[tokio::test]
async fn people_who_cannot_see_the_issue_get_no_mail() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let (_, account) = app.user("dev@tessera.test", "Dev").await;
    let (_, scheme) = app
        .post(
            &format!("{API}/permissionscheme"),
            json!({ "name": "Admins only", "permissions": [
                { "holder": { "type": "projectRole", "parameter": "10002" }, "permission": "BROWSE_PROJECTS" },
            ] }),
        )
        .await;
    app.put(
        &format!("{API}/project/PROJ/permissionscheme"),
        json!({ "id": scheme["id"] }),
    )
    .await;
    app.issue(
        "PROJ",
        "Secret",
        json!({ "assignee": { "accountId": account } }),
    )
    .await;
    assert!(outbox(&app).await.is_empty());
}

#[tokio::test]
async fn confluence_watchers_hear_about_edits_and_comments() {
    let app = TestApp::new().await;
    let wiki = "/wiki/rest/api";
    app.post(
        &format!("{wiki}/space"),
        json!({ "key": "DOCS", "name": "Docs" }),
    )
    .await;
    let (_, page) = app
        .post(
            &format!("{wiki}/content"),
            json!({ "type": "page", "title": "Guide", "space": { "key": "DOCS" } }),
        )
        .await;
    let page_id = page["id"].as_str().unwrap().to_string();
    let (dev, _) = app.user("dev@tessera.test", "Dev").await;

    // The dev watches the page; the admin wrote it and watches it already.
    let (status, _) = app
        .request(
            Method::POST,
            &format!("{wiki}/user/watch/content/{page_id}"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, watching) = app
        .request(
            Method::GET,
            &format!("{wiki}/user/watch/content/{page_id}"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(watching["watching"], true);
    clear(&app).await;

    app.put(
        &format!("{wiki}/content/{page_id}"),
        json!({ "type": "page", "title": "Guide", "version": { "number": 2 }, "body": { "storage": { "value": "<p>New</p>", "representation": "storage" } } }),
    )
    .await;
    let mail = outbox(&app).await;
    assert_eq!(mail.len(), 1, "only the dev; the admin made the change");
    assert_eq!(mail[0].0, "dev@tessera.test");
    assert_eq!(mail[0].1, "[Tessera] Docs > Guide");
    assert!(mail[0].2.contains("Ada Admin edited Guide"));
    clear(&app).await;

    // Minor edits notify nobody.
    app.put(
        &format!("{wiki}/content/{page_id}"),
        json!({ "type": "page", "title": "Guide", "version": { "number": 3, "minorEdit": true } }),
    )
    .await;
    assert!(outbox(&app).await.is_empty());

    // The dev's comment reaches the admin, who watches what they created.
    app.request(
        Method::POST,
        &format!("{wiki}/content"),
        Some(&dev),
        Some(json!({ "type": "comment", "container": { "id": page_id }, "body": { "storage": { "value": "<p>Looks right</p>", "representation": "storage" } } })),
    )
    .await;
    let mail = outbox(&app).await;
    assert_eq!(mail.len(), 1);
    assert_eq!(mail[0].0, "admin@tessera.test");
    assert!(mail[0].2.contains("Dev commented on Guide"));
    assert!(mail[0].2.contains("Looks right"));
}
