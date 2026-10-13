//! The Confluence v2 REST API: spaces, pages, bodies in storage and ADF, versions, the page
//! tree, footer comments, and cursor paging.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::{Value, json};

const V2: &str = "/wiki/api/v2";

/// A space, and its id.
async fn space(app: &TestApp, key: &str) -> String {
    let (status, space) = app
        .post(
            &format!("{V2}/spaces"),
            json!({ "key": key, "name": format!("{key} space") }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{space}");
    space["id"]
        .as_str()
        .expect("v2 ids are strings")
        .to_string()
}

async fn page(app: &TestApp, space_id: &str, title: &str, parent: Option<&str>) -> Value {
    let mut body = json!({
        "spaceId": space_id,
        "status": "current",
        "title": title,
        "body": { "representation": "storage", "value": format!("<p>{title}</p>") },
    });
    if let Some(parent) = parent {
        body["parentId"] = json!(parent);
    }
    let (status, page) = app.post(&format!("{V2}/pages"), body).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page
}

fn id(value: &Value) -> String {
    value["id"].as_str().unwrap().to_string()
}

fn titles(page: &Value) -> Vec<String> {
    page["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn spaces_and_pages() {
    let app = TestApp::new().await;
    let space_id = space(&app, "DOCS").await;
    let (_, read) = app.get(&format!("{V2}/spaces/{space_id}")).await;
    assert_eq!(read["key"], "DOCS");
    let home = read["homepageId"].as_str().unwrap().to_string();

    // Without a parent, a page goes under the home page.
    let created = page(&app, &space_id, "Guide", None).await;
    assert_eq!(created["parentId"], json!(home));
    assert_eq!(created["version"]["number"], 1);
    assert_eq!(created["body"]["storage"]["value"], "<p>Guide</p>");

    // With `root-level`, it goes to the top of the space.
    let (_, top) = app
        .post(
            &format!("{V2}/pages?root-level=true"),
            json!({ "spaceId": space_id, "title": "Top", "body": { "representation": "storage", "value": "" } }),
        )
        .await;
    assert_eq!(top["parentId"], Value::Null);

    let (_, listed) = app
        .get(&format!("{V2}/spaces/{space_id}/pages?depth=root"))
        .await;
    assert_eq!(titles(&listed), ["DOCS space Home", "Top"]);

    let (_, keys) = app.get(&format!("{V2}/spaces?keys=DOCS")).await;
    assert_eq!(keys["results"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn bodies_in_storage_and_adf() {
    let app = TestApp::new().await;
    let space_id = space(&app, "DOCS").await;
    let adf = json!({
        "type": "doc", "version": 1,
        "content": [
            { "type": "heading", "attrs": { "level": 2 }, "content": [{ "type": "text", "text": "Setup" }] },
            { "type": "paragraph", "content": [{ "type": "text", "text": "Run ", "marks": [] }, { "type": "text", "text": "make", "marks": [{ "type": "code" }] }] },
        ],
    });
    let (status, created) = app
        .post(
            &format!("{V2}/pages"),
            json!({ "spaceId": space_id, "title": "ADF page", "body": { "representation": "atlas_doc_format", "value": adf.to_string() } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let page_id = id(&created);

    let (_, storage) = app
        .get(&format!("{V2}/pages/{page_id}?body-format=storage"))
        .await;
    assert_eq!(
        storage["body"]["storage"]["value"],
        "<h2>Setup</h2><p>Run <code>make</code></p>"
    );

    let (_, as_adf) = app
        .get(&format!(
            "{V2}/pages/{page_id}?body-format=atlas_doc_format"
        ))
        .await;
    let value: Value = serde_json::from_str(
        as_adf["body"]["atlas_doc_format"]["value"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["content"][0]["type"], "heading");
    assert_eq!(
        value["content"][1]["content"][1]["marks"][0]["type"],
        "code"
    );

    // v1 serves the same body as ADF too.
    let (_, v1) = app
        .get(&format!(
            "/wiki/rest/api/content/{page_id}?expand=body.atlas_doc_format"
        ))
        .await;
    assert!(
        v1["body"]["atlas_doc_format"]["value"]
            .as_str()
            .unwrap()
            .contains("\"heading\"")
    );

    let (_, none) = app.get(&format!("{V2}/pages/{page_id}")).await;
    assert_eq!(none["body"], json!({}));
}

#[tokio::test]
async fn versions_and_conflicts() {
    let app = TestApp::new().await;
    let space_id = space(&app, "DOCS").await;
    let page_id = id(&page(&app, &space_id, "Guide", None).await);

    let update = |number: i64, title: &str| {
        json!({
            "id": page_id, "status": "current", "title": title,
            "body": { "representation": "storage", "value": format!("<p>{title}</p>") },
            "version": { "number": number, "message": "edit" },
        })
    };
    let (status, error) = app
        .put(&format!("{V2}/pages/{page_id}"), update(1, "Stale"))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["errors"][0]["code"], "CONFLICT");
    assert_eq!(error["errors"][0]["status"], 409);

    let (status, updated) = app
        .put(&format!("{V2}/pages/{page_id}"), update(2, "Guide v2"))
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["version"]["number"], 2);

    let (_, versions) = app.get(&format!("{V2}/pages/{page_id}/versions")).await;
    let numbers: Vec<i64> = versions["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["number"].as_i64().unwrap())
        .collect();
    assert_eq!(numbers, [2, 1]);
    let (_, first) = app.get(&format!("{V2}/pages/{page_id}/versions/1")).await;
    assert_eq!(first["nextVersion"]["number"], 2);

    let (_, old) = app
        .get(&format!(
            "{V2}/pages/{page_id}?version=1&body-format=storage"
        ))
        .await;
    assert_eq!(old["title"], "Guide");
    assert_eq!(old["body"]["storage"]["value"], "<p>Guide</p>");

    let (status, renamed) = app
        .put(
            &format!("{V2}/pages/{page_id}/title"),
            json!({ "status": "current", "title": "Renamed" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["version"]["number"], 3);
}

#[tokio::test]
async fn tree_comments_and_paging() {
    let app = TestApp::new().await;
    let space_id = space(&app, "DOCS").await;
    let parent = id(&page(&app, &space_id, "Parent", None).await);
    let child = id(&page(&app, &space_id, "Child", Some(&parent)).await);
    page(&app, &space_id, "Grandchild", Some(&child)).await;

    let (_, kids) = app.get(&format!("{V2}/pages/{parent}/children")).await;
    assert_eq!(titles(&kids), ["Child"]);
    let (_, below) = app.get(&format!("{V2}/pages/{parent}/descendants")).await;
    assert_eq!(titles(&below), ["Child", "Grandchild"]);
    assert_eq!(below["results"][1]["depth"], 2);
    let (_, up) = app.get(&format!("{V2}/pages/{child}/ancestors")).await;
    assert_eq!(
        up["results"].as_array().unwrap().len(),
        2,
        "home page and parent"
    );

    let (status, comment) = app
        .post(
            &format!("{V2}/footer-comments"),
            json!({ "pageId": parent, "body": { "representation": "storage", "value": "<p>First</p>" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{comment}");
    assert_eq!(comment["pageId"], json!(parent));
    let comment_id = id(&comment);
    let (_, reply) = app
        .post(
            &format!("{V2}/footer-comments"),
            json!({ "parentCommentId": comment_id, "body": { "representation": "storage", "value": "<p>Reply</p>" } }),
        )
        .await;
    assert_eq!(reply["parentCommentId"], json!(comment_id));
    let (_, top) = app
        .get(&format!(
            "{V2}/pages/{parent}/footer-comments?body-format=storage"
        ))
        .await;
    assert_eq!(top["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        top["results"][0]["body"]["storage"]["value"],
        "<p>First</p>"
    );
    let (_, replies) = app
        .get(&format!("{V2}/footer-comments/{comment_id}/children"))
        .await;
    assert_eq!(replies["results"].as_array().unwrap().len(), 1);

    // Cursor paging: two pages of two, then one.
    let (_, first) = app.get(&format!("{V2}/pages?limit=2&sort=title")).await;
    assert_eq!(titles(&first), ["Child", "DOCS space Home"]);
    let next = first["_links"]["next"]
        .as_str()
        .expect("a next link")
        .to_string();
    let (_, second) = app.get(&next).await;
    assert_eq!(titles(&second), ["Grandchild", "Parent"]);
    let (status, error) = app.get(&format!("{V2}/pages?cursor=nonsense")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["errors"][0]["code"], "INVALID_REQUEST_PARAMETER");

    // Trash, then purge.
    let (status, _) = app.delete(&format!("{V2}/pages/{child}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app.get(&format!("{V2}/pages/{child}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app.delete(&format!("{V2}/pages/{child}?purge=true")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app.get(&format!("{V2}/pages/{child}?status=trashed")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
