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

#[tokio::test]
async fn view_draws_macros_and_mentions() {
    let app = TestApp::new().await;
    space(&app, "DOCS").await;
    let (_, me) = app.get(&format!("{API}/user/current")).await;
    let account = me["accountId"].as_str().unwrap();
    let name = me["displayName"].as_str().unwrap();
    let storage = format!(
        "<ac:structured-macro ac:name=\"code\"><ac:plain-text-body><![CDATA[if a > b {{}}]]></ac:plain-text-body></ac:structured-macro><p>Ask <ac:link><ri:user ri:account-id=\"{account}\" /></ac:link></p>"
    );
    let (_, created) = app
        .post(
            &format!("{API}/content?expand=body.view"),
            json!({ "type": "page", "title": "Macros", "space": { "key": "DOCS" }, "body": { "storage": { "value": storage, "representation": "storage" } } }),
        )
        .await;
    assert_eq!(
        created["body"]["view"]["value"],
        format!(
            "<pre><code>if a &gt; b {{}}</code></pre><p>Ask <span class=\"mention\">@{name}</span></p>"
        )
    );
    // Code inside macros is searchable.
    let (_, found) = search(&app, "text ~ \"if a\"").await;
    assert_eq!(titles(&found), ["Macros"]);
}

/// A multipart body with one file part, and its content type.
fn upload(
    filename: &str,
    media_type: &str,
    bytes: &str,
    comment: Option<&str>,
) -> (Vec<u8>, String) {
    let boundary = "tessera-test-boundary";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
         Content-Type: {media_type}\r\n\r\n{bytes}\r\n"
    );
    if let Some(comment) = comment {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"comment\"\r\n\r\n{comment}\r\n"
        ));
    }
    body.push_str(&format!("--{boundary}--\r\n"));
    (
        body.into_bytes(),
        format!("multipart/form-data; boundary={boundary}"),
    )
}

#[tokio::test]
async fn attachments_keep_versions() {
    use axum::http::Method;
    let app = TestApp::new().await;
    space(&app, "DOCS").await;
    let page_id = id(&page(&app, "DOCS", "Guide", None).await);
    let path = format!("{API}/content/{page_id}/child/attachment");

    let (body, content_type) = upload(
        "arch diagram.png",
        "image/png",
        "PNGDATA",
        Some("First draft"),
    );
    let (status, _, _) = app
        .raw(
            Method::POST,
            &path,
            &[("content-type", &content_type)],
            body.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "the XSRF header is required");

    let headers = [
        ("content-type", content_type.as_str()),
        ("X-Atlassian-Token", "no-check"),
    ];
    let (status, _, bytes) = app.raw(Method::POST, &path, &headers, body.clone()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let created: Value = serde_json::from_slice(&bytes).unwrap();
    let attachment = &created["results"][0];
    let attachment_id = attachment["id"].as_str().unwrap().to_string();
    assert!(attachment_id.starts_with("att"));
    assert_eq!(attachment["title"], "arch diagram.png");
    assert_eq!(attachment["extensions"]["mediaType"], "image/png");
    assert_eq!(attachment["extensions"]["fileSize"], 7);
    assert_eq!(attachment["extensions"]["comment"], "First draft");

    // The same name again is refused by POST, and becomes version 2 through the data endpoint.
    let (status, _, _) = app.raw(Method::POST, &path, &headers, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (body, content_type) = upload("arch diagram.png", "image/png", "NEWER", None);
    let headers2 = [
        ("content-type", content_type.as_str()),
        ("X-Atlassian-Token", "no-check"),
    ];
    let (status, _, bytes) = app
        .raw(
            Method::POST,
            &format!("{path}/{attachment_id}/data"),
            &headers2,
            body,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let updated: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(updated["extensions"]["fileSize"], 5);

    // Downloads: the latest version by default, older ones by number. Images show inline.
    let download = updated["_links"]["download"]
        .as_str()
        .unwrap()
        .split('?')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(
        download,
        format!("/download/attachments/{page_id}/arch%20diagram%2Epng")
    );
    let (status, headers, bytes) = app
        .raw(Method::GET, &format!("/wiki{download}"), &[], Vec::new())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"NEWER");
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("inline")
    );
    let (_, _, bytes) = app
        .raw(
            Method::GET,
            &format!("/wiki{download}?version=1"),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(bytes, b"PNGDATA");

    // Files that could run script are always downloaded, never shown.
    let (body, content_type) = upload("page.html", "text/html", "<script>alert(1)</script>", None);
    let headers3 = [
        ("content-type", content_type.as_str()),
        ("X-Atlassian-Token", "no-check"),
    ];
    app.raw(Method::POST, &path, &headers3, body).await;
    let (_, headers, _) = app
        .raw(
            Method::GET,
            &format!("/wiki/download/attachments/{page_id}/page.html"),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(headers["content-type"], "application/octet-stream");
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );

    // Listing, filtering, and v2.
    let (_, list) = app.get(&format!("{path}?filename=page.html")).await;
    assert_eq!(list["size"], 1);
    let (_, v2) = app
        .get(&format!("/wiki/api/v2/pages/{page_id}/attachments"))
        .await;
    assert_eq!(v2["results"].as_array().unwrap().len(), 2);
    assert_eq!(v2["results"][0]["pageId"], json!(page_id));

    // Images in the page body point at the attachment.
    let (_, read) = app
        .get(&format!("{API}/content/{page_id}?expand=version"))
        .await;
    let number = read["version"]["number"].as_i64().unwrap() + 1;
    let (status, edited) = app
        .put(
            &format!("{API}/content/{page_id}?expand=body.view"),
            json!({ "type": "page", "title": "Guide", "version": { "number": number },
                    "body": { "storage": { "representation": "storage", "value": "<ac:image><ri:attachment ri:filename=\"arch diagram.png\" /></ac:image>" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(
        edited["body"]["view"]["value"],
        format!(
            "<p class=\"media\"><img src=\"/wiki/download/attachments/{page_id}/arch%20diagram%2Epng\" alt=\"\"></p>"
        )
    );

    // Trash, then purge, removes the attachment.
    let (status, _) = app.delete(&format!("{API}/content/{attachment_id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .delete(&format!("{API}/content/{attachment_id}?status=trashed"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = app.get(&path).await;
    assert_eq!(list["size"], 1);
}
