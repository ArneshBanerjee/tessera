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

/// A comment body that mentions one account.
fn mention_doc(account: &str, name: &str, text: &str) -> serde_json::Value {
    json!({ "type": "doc", "version": 1, "content": [{ "type": "paragraph", "content": [
        { "type": "mention", "attrs": { "id": account, "text": format!("@{name}") } },
        { "type": "text", "text": text },
    ] }] })
}

#[tokio::test]
async fn mentions_reach_people_and_show_in_the_app() {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let (dev, dev_account) = app.user("dev@tessera.test", "Dev").await;
    let (ops, ops_account) = app.user("ops@tessera.test", "Ops").await;
    let key = app.issue("PROJ", "Slow search", json!({})).await;
    clear(&app).await;

    // Dev, who does not watch the issue, is mentioned in a comment.
    let (status, comment) = app
        .post(
            &format!("{API}/issue/{key}/comment"),
            json!({ "body": mention_doc(&dev_account, "Dev", " can you look?") }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let mail = outbox(&app).await;
    assert_eq!(mail.len(), 1, "{mail:?}");
    assert_eq!(mail[0].0, "dev@tessera.test");
    assert_eq!(
        mail[0].1,
        format!("[Tessera] Ada Admin mentioned you on {key}")
    );

    // It shows in Dev's notifications, unread.
    let (_, notes) = app
        .request(
            Method::GET,
            "/rest/tessera/1.0/notifications",
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(notes["unread"], 1);
    let note = &notes["values"][0];
    assert_eq!(note["kind"], "mention");
    assert_eq!(note["link"], format!("/browse/{key}"));
    assert_eq!(note["actor"]["displayName"], "Ada Admin");
    assert_eq!(note["read"], false);
    let id = note["id"].as_str().unwrap();
    let (status, _) = app
        .request(
            Method::POST,
            &format!("/rest/tessera/1.0/notifications/{id}/read"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, notes) = app
        .request(
            Method::GET,
            "/rest/tessera/1.0/notifications",
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(notes["unread"], 0);
    // Nobody else can mark it.
    let (status, _) = app
        .request(
            Method::POST,
            &format!("/rest/tessera/1.0/notifications/{id}/read"),
            Some(&ops),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    clear(&app).await;

    // Editing the comment to add Ops mentions only Ops; Dev is not told again.
    let comment_id = comment["id"].as_str().unwrap();
    let mut body = mention_doc(&dev_account, "Dev", " and ");
    body["content"][0]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "type": "mention", "attrs": { "id": ops_account, "text": "@Ops" } }));
    let (status, _) = app
        .put(
            &format!("{API}/issue/{key}/comment/{comment_id}"),
            json!({ "body": body }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let mail = outbox(&app).await;
    assert_eq!(mail.len(), 1, "{mail:?}");
    assert_eq!(mail[0].0, "ops@tessera.test");
    clear(&app).await;

    // A description mention on a new issue reaches the person mentioned.
    let created = app
        .issue(
            "PROJ",
            "Disk full",
            json!({ "description": mention_doc(&ops_account, "Ops", " please check") }),
        )
        .await;
    let mail = outbox(&app).await;
    assert_eq!(mail.len(), 1, "{mail:?}");
    assert_eq!(
        mail[0].1,
        format!("[Tessera] Ada Admin mentioned you on {created}")
    );

    // Read-all clears the count.
    let (_, notes) = app
        .request(
            Method::GET,
            "/rest/tessera/1.0/notifications",
            Some(&ops),
            None,
        )
        .await;
    assert_eq!(notes["unread"], 2);
    app.request(
        Method::POST,
        "/rest/tessera/1.0/notifications/read-all",
        Some(&ops),
        None,
    )
    .await;
    let (_, notes) = app
        .request(
            Method::GET,
            "/rest/tessera/1.0/notifications",
            Some(&ops),
            None,
        )
        .await;
    assert_eq!(notes["unread"], 0);
}

#[tokio::test]
async fn page_mentions_reach_people() {
    let app = TestApp::new().await;
    app.post(
        "/wiki/rest/api/space",
        json!({ "key": "DOCS", "name": "Docs" }),
    )
    .await;
    let (dev, dev_account) = app.user("dev@tessera.test", "Dev").await;
    clear(&app).await;
    let storage =
        format!("<p><ac:link><ri:user ri:account-id=\"{dev_account}\" /></ac:link> owns this.</p>");
    let (status, page) = app
        .post(
            "/wiki/rest/api/content",
            json!({ "type": "page", "title": "Ownership", "space": { "key": "DOCS" },
                    "body": { "storage": { "value": storage, "representation": "storage" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let mail = outbox(&app).await;
    assert_eq!(mail.len(), 1, "{mail:?}");
    assert_eq!(mail[0].0, "dev@tessera.test");
    assert_eq!(mail[0].1, "[Tessera] Ada Admin mentioned you on Ownership");
    let (_, notes) = app
        .request(
            Method::GET,
            "/rest/tessera/1.0/notifications",
            Some(&dev),
            None,
        )
        .await;
    let id = page["id"].as_str().unwrap();
    assert_eq!(
        notes["values"][0]["link"],
        format!("/wiki/spaces/DOCS/pages/{id}")
    );
}
