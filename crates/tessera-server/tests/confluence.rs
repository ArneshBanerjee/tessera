//! The Confluence v1 REST API: spaces, pages, versions, the page tree, comments, labels, and CQL.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use serde_json::{Value, json};

const API: &str = "/wiki/rest/api";

async fn space(app: &TestApp, key: &str) -> Value {
    let (status, space) = app
        .post(
            &format!("{API}/space"),
            json!({ "key": key, "name": format!("{key} space"), "description": { "plain": { "value": "Docs", "representation": "plain" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{space}");
    space
}

async fn page(app: &TestApp, space: &str, title: &str, parent: Option<&str>) -> Value {
    let mut body = json!({
        "type": "page",
        "title": title,
        "space": { "key": space },
        "body": { "storage": { "value": format!("<p>About {title}</p>"), "representation": "storage" } },
    });
    if let Some(parent) = parent {
        body["ancestors"] = json!([{ "id": parent }]);
    }
    let (status, page) = app.post(&format!("{API}/content"), body).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page
}

fn id(value: &Value) -> String {
    value["id"]
        .as_str()
        .expect("content ids are strings")
        .to_string()
}

fn titles(collection: &Value) -> Vec<String> {
    collection["results"]
        .as_array()
        .expect("a collection")
        .iter()
        .map(|r| r["title"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn spaces_have_home_pages() {
    let app = TestApp::new().await;
    let created = space(&app, "DOCS").await;
    assert_eq!(created["key"], "DOCS");
    assert!(created["id"].is_number());

    let (_, read) = app
        .get(&format!(
            "{API}/space/DOCS?expand=description.plain,homepage"
        ))
        .await;
    assert_eq!(read["description"]["plain"]["value"], "Docs");
    assert_eq!(read["homepage"]["title"], "DOCS space Home");

    let (status, error) = app
        .post(
            &format!("{API}/space"),
            json!({ "key": "DOCS", "name": "Again" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["statusCode"], 400);

    let (_, list) = app
        .get(&format!("{API}/space?spaceKey=DOCS&spaceKey=NONE"))
        .await;
    assert_eq!(list["size"], 1);

    let (status, _) = app.get(&format!("{API}/space/NONE")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pages_keep_every_version() {
    let app = TestApp::new().await;
    space(&app, "DOCS").await;
    let created = page(&app, "DOCS", "Guide", None).await;
    let page_id = id(&created);
    assert_eq!(created["version"]["number"], 1);
    assert_eq!(created["body"]["storage"]["value"], "<p>About Guide</p>");

    // A stale version number is a conflict.
    let (status, error) = app
        .put(
            &format!("{API}/content/{page_id}"),
            json!({ "type": "page", "title": "Guide", "version": { "number": 1 }, "body": { "storage": { "value": "<p>x</p>", "representation": "storage" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        error["message"],
        "Version must be incremented on update. Current version is: 1"
    );

    let (status, updated) = app
        .put(
            &format!("{API}/content/{page_id}"),
            json!({ "type": "page", "title": "User guide", "version": { "number": 2, "message": "Rename" }, "body": { "storage": { "value": "<p>New text</p>", "representation": "storage" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["version"]["number"], 2);
    assert_eq!(updated["version"]["message"], "Rename");

    let (_, old) = app
        .get(&format!(
            "{API}/content/{page_id}?version=1&expand=body.storage"
        ))
        .await;
    assert_eq!(old["title"], "Guide");
    assert_eq!(old["status"], "historical");
    assert_eq!(old["body"]["storage"]["value"], "<p>About Guide</p>");

    let (_, versions) = app.get(&format!("{API}/content/{page_id}/version")).await;
    assert_eq!(versions["size"], 2);

    // Restoring version 1 saves it as version 3.
    let (status, restored) = app
        .post(
            &format!("{API}/content/{page_id}/version"),
            json!({ "operationKey": "restore", "params": { "versionNumber": 1, "message": "Back", "restoreTitle": true } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(restored["number"], 3);
    let (_, current) = app
        .get(&format!("{API}/content/{page_id}?expand=body.view,version"))
        .await;
    assert_eq!(current["title"], "Guide");
    assert_eq!(current["body"]["view"]["value"], "<p>About Guide</p>");

    // Titles are unique in a space.
    let (status, error) = app
        .post(
            &format!("{API}/content"),
            json!({ "type": "page", "title": "Guide", "space": { "key": "DOCS" } }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("A page with this title already exists")
    );
}

#[tokio::test]
async fn the_page_tree() {
    let app = TestApp::new().await;
    space(&app, "DOCS").await;
    let parent = id(&page(&app, "DOCS", "Parent", None).await);
    let first = id(&page(&app, "DOCS", "First", Some(&parent)).await);
    let second = id(&page(&app, "DOCS", "Second", Some(&parent)).await);
    let grandchild = id(&page(&app, "DOCS", "Grandchild", Some(&first)).await);

    let (_, children) = app.get(&format!("{API}/content/{parent}/child/page")).await;
    assert_eq!(titles(&children), ["First", "Second"]);

    let (_, descendants) = app
        .get(&format!("{API}/content/{parent}/descendant/page"))
        .await;
    assert_eq!(titles(&descendants), ["First", "Grandchild", "Second"]);

    let (_, read) = app
        .get(&format!("{API}/content/{grandchild}?expand=ancestors"))
        .await;
    let ancestors: Vec<&str> = read["ancestors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect();
    assert_eq!(ancestors, ["Parent", "First"]);

    // Move Second before First.
    let (status, _) = app
        .put(
            &format!("{API}/content/{second}/move/before/{first}"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, children) = app.get(&format!("{API}/content/{parent}/child/page")).await;
    assert_eq!(titles(&children), ["Second", "First"]);

    // A page cannot move below its own child.
    let (status, _) = app
        .put(
            &format!("{API}/content/{parent}/move/append/{first}"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Trashing a page moves its children up to its parent.
    let (status, _) = app.delete(&format!("{API}/content/{first}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, children) = app.get(&format!("{API}/content/{parent}/child/page")).await;
    assert_eq!(titles(&children), ["Second", "Grandchild"]);
    let (status, _) = app.get(&format!("{API}/content/{first}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, trashed) = app
        .get(&format!("{API}/content/{first}?status=trashed"))
        .await;
    assert_eq!(trashed["status"], "trashed");

    // Purge it for good.
    let (status, _) = app
        .delete(&format!("{API}/content/{first}?status=trashed"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .get(&format!("{API}/content/{first}?status=trashed"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn comments_and_labels() {
    let app = TestApp::new().await;
    space(&app, "DOCS").await;
    let page_id = id(&page(&app, "DOCS", "Guide", None).await);

    let (status, comment) = app
        .post(
            &format!("{API}/content"),
            json!({ "type": "comment", "container": { "id": page_id, "type": "page" }, "body": { "storage": { "value": "<p>Nice</p>", "representation": "storage" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{comment}");
    let comment_id = id(&comment);
    let (status, _) = app
        .post(
            &format!("{API}/content"),
            json!({ "type": "comment", "container": { "id": page_id }, "ancestors": [{ "id": comment_id }], "body": { "storage": { "value": "<p>Thanks</p>", "representation": "storage" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, top) = app
        .get(&format!("{API}/content/{page_id}/child/comment"))
        .await;
    assert_eq!(top["size"], 1);
    let (_, all) = app
        .get(&format!("{API}/content/{page_id}/child/comment?depth=all"))
        .await;
    assert_eq!(all["size"], 2);

    let (status, labels) = app
        .post(
            &format!("{API}/content/{page_id}/label"),
            json!([{ "prefix": "global", "name": "How-To" }, { "name": "draft" }]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{labels}");
    let names: Vec<&str> = labels["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["draft", "how-to"]);
    let (status, _) = app
        .delete(&format!("{API}/content/{page_id}/label/draft"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .delete(&format!("{API}/content/{page_id}/label?name=draft"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, read) = app
        .get(&format!("{API}/content/{page_id}?expand=metadata.labels"))
        .await;
    assert_eq!(read["metadata"]["labels"]["results"][0]["name"], "how-to");
}

async fn search(app: &TestApp, cql: &str) -> (StatusCode, Value) {
    let query = form_urlencoded::byte_serialize(cql.as_bytes()).collect::<String>();
    app.get(&format!("{API}/content/search?cql={query}")).await
}

#[tokio::test]
async fn cql_search() {
    let app = TestApp::new().await;
    space(&app, "DOCS").await;
    space(&app, "ENG").await;
    let guide = id(&page(&app, "DOCS", "Install guide", None).await);
    page(&app, "DOCS", "Release notes", Some(&guide)).await;
    page(&app, "ENG", "Install scripts", None).await;
    app.post(
        &format!("{API}/content/{guide}/label"),
        json!({ "name": "howto" }),
    )
    .await;

    let (_, found) = search(&app, "space = DOCS and type = page and title ~ install").await;
    assert_eq!(titles(&found), ["Install guide"]);
    let (_, found) = search(&app, "text ~ \"about release\"").await;
    assert_eq!(titles(&found), ["Release notes"]);
    let (_, found) = search(&app, &format!("ancestor = {guide}")).await;
    assert_eq!(titles(&found), ["Release notes"]);
    let (_, found) = search(&app, "label = howto").await;
    assert_eq!(titles(&found), ["Install guide"]);
    let (_, found) = search(
        &app,
        "title ~ install and creator = currentUser() order by title desc",
    )
    .await;
    assert_eq!(titles(&found), ["Install scripts", "Install guide"]);
    let (_, found) = search(
        &app,
        "created > now(\"-1d\") and space.key in (ENG) and title ~ install",
    )
    .await;
    assert_eq!(titles(&found), ["Install scripts"]);

    let (status, error) = search(&app, "colour = red").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("Could not parse cql : colour = red")
    );

    // `/search` wraps results, and can find spaces.
    let (_, results) = app
        .get(&format!("{API}/search?cql=type%3Dspace%20and%20title~eng"))
        .await;
    assert_eq!(results["totalSize"], 1);
    assert_eq!(results["results"][0]["entityType"], "space");
    assert_eq!(results["results"][0]["space"]["key"], "ENG");
    let (_, results) = app.get(&format!("{API}/search?cql=title~notes")).await;
    assert_eq!(results["results"][0]["content"]["title"], "Release notes");
    assert_eq!(results["results"][0]["excerpt"], "About Release notes");
}

#[tokio::test]
async fn view_body_is_sanitized() {
    let app = TestApp::new().await;
    space(&app, "DOCS").await;
    let (_, created) = app
        .post(
            &format!("{API}/content?expand=body.view"),
            json!({ "type": "page", "title": "Unsafe", "space": { "key": "DOCS" }, "body": { "storage": { "value": "<p onclick=\"x()\">Hi</p><script>alert(1)</script>", "representation": "storage" } } }),
        )
        .await;
    assert_eq!(created["body"]["view"]["value"], "<p>Hi</p>");
}
