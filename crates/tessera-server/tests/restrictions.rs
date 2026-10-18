//! Confluence page restrictions: who may view and who may edit a page.

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::{Value, json};

const API: &str = "/wiki/rest/api";

async fn page(app: &TestApp, title: &str, parent: Option<&str>) -> String {
    let mut body = json!({
        "type": "page",
        "title": title,
        "space": { "key": "DOCS" },
        "body": { "storage": { "value": "<p>Text</p>", "representation": "storage" } },
    });
    if let Some(parent) = parent {
        body["ancestors"] = json!([{ "id": parent }]);
    }
    let (status, page) = app.post(&format!("{API}/content"), body).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page["id"].as_str().unwrap().to_string()
}

fn titles(collection: &Value) -> Vec<String> {
    let mut out: Vec<String> = collection["results"]
        .as_array()
        .expect("a collection")
        .iter()
        .map(|r| r["title"].as_str().unwrap_or_default().to_string())
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn view_restrictions_hide_a_page_and_its_children() {
    let app = TestApp::new().await;
    app.post(
        &format!("{API}/space"),
        json!({ "key": "DOCS", "name": "Docs" }),
    )
    .await;
    let secret = page(&app, "Secret", None).await;
    let below = page(&app, "Below secret", Some(&secret)).await;
    page(&app, "Open", None).await;
    let (dev, dev_account) = app.user("dev@tessera.test", "Dev").await;
    let (ops, ops_account) = app.user("ops@tessera.test", "Ops").await;
    let as_user = |auth: &str, method: Method, path: String, body: Option<Value>| {
        let auth = auth.to_string();
        let app = &app;
        async move { app.request(method, &path, Some(&auth), body).await }
    };

    // Only Ops may view the secret page.
    let (status, set) = app
        .put(
            &format!("{API}/content/{secret}/restriction"),
            json!([{ "operation": "read", "restrictions": { "user": [{ "type": "known", "accountId": ops_account }] } }]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert_eq!(set["results"][0]["operation"], "read");
    assert_eq!(
        set["results"][0]["restrictions"]["user"]["results"][0]["accountId"],
        json!(ops_account)
    );

    // For Dev, the page and the page below it do not exist.
    for id in [&secret, &below] {
        let (status, _) = as_user(&dev, Method::GET, format!("{API}/content/{id}"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = as_user(&ops, Method::GET, format!("{API}/content/{id}"), None).await;
        assert_eq!(status, StatusCode::OK);
    }
    let (_, listed) = as_user(
        &dev,
        Method::GET,
        format!("{API}/space/DOCS/content/page"),
        None,
    )
    .await;
    assert!(!titles(&listed).contains(&"Secret".to_string()), "{listed}");
    assert!(!titles(&listed).contains(&"Below secret".to_string()));
    let (_, found) = as_user(
        &dev,
        Method::GET,
        format!("{API}/content/search?cql=type%3Dpage"),
        None,
    )
    .await;
    assert!(!titles(&found).contains(&"Secret".to_string()));
    let (_, found) = as_user(
        &ops,
        Method::GET,
        format!("{API}/content/search?cql=type%3Dpage"),
        None,
    )
    .await;
    assert!(titles(&found).contains(&"Below secret".to_string()));

    // v2 lists leave the hidden pages out too.
    let (_, v2) = as_user(
        &dev,
        Method::GET,
        "/wiki/api/v2/pages?limit=50".into(),
        None,
    )
    .await;
    assert!(!titles(&v2).contains(&"Secret".to_string()), "{v2}");

    // The restrictions show by operation, and through the expansion.
    let (_, by_op) = app
        .get(&format!("{API}/content/{secret}/restriction/byOperation"))
        .await;
    assert_eq!(by_op["read"]["restrictions"]["user"]["size"], 1);
    assert_eq!(by_op["update"]["restrictions"]["user"]["size"], 0);
    let (_, expanded) = app
        .get(&format!(
            "{API}/content/{secret}?expand=restrictions.read.restrictions.user"
        ))
        .await;
    assert_eq!(
        expanded["restrictions"]["read"]["restrictions"]["user"]["size"],
        1
    );

    // Watchers of the space hear about edits only to pages they can see.
    for who in [&dev, &ops] {
        let (status, _) = as_user(
            who,
            Method::POST,
            format!("{API}/user/watch/space/DOCS"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let (status, _) = app
        .put(
            &format!("{API}/content/{below}"),
            json!({ "type": "page", "title": "Below secret", "version": { "number": 2 } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let mailed: Vec<String> = sqlx::query_scalar("SELECT to_email FROM outbox")
        .fetch_all(&app.state.db)
        .await
        .unwrap();
    assert_eq!(mailed, ["ops@tessera.test"]);

    // Adding Dev by account id lets them see it.
    let (status, _) = app
        .put(
            &format!(
                "{API}/content/{secret}/restriction/byOperation/read/user?accountId={dev_account}"
            ),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = as_user(&dev, Method::GET, format!("{API}/content/{below}"), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = app
        .get(&format!(
            "{API}/content/{secret}/restriction/byOperation/read/user?accountId={dev_account}"
        ))
        .await;
    assert_eq!(status, StatusCode::OK);

    // Clearing the restrictions opens the page to the space again.
    let (status, cleared) = app
        .delete(&format!("{API}/content/{secret}/restriction"))
        .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    let (_, by_op) = app
        .get(&format!("{API}/content/{secret}/restriction/byOperation"))
        .await;
    assert_eq!(by_op["read"]["restrictions"]["user"]["size"], 0);
}

#[tokio::test]
async fn edit_restrictions_limit_who_changes_a_page() {
    let app = TestApp::new().await;
    app.post(
        &format!("{API}/space"),
        json!({ "key": "DOCS", "name": "Docs" }),
    )
    .await;
    let id = page(&app, "Policy", None).await;
    let (dev, dev_account) = app.user("dev@tessera.test", "Dev").await;
    let (ops, ops_account) = app.user("ops@tessera.test", "Ops").await;
    app.post("/rest/api/3/group", json!({ "name": "editors" }))
        .await;
    let (status, added) = app
        .post(
            "/rest/api/3/group/user?groupname=editors",
            json!({ "accountId": ops_account }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{added}");

    // Ops restricts editing to the editors group, a group they are in.
    let (status, set) = app
        .request(
            Method::PUT,
            &format!("{API}/content/{id}/restriction/byOperation/update/group/editors"),
            Some(&ops),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    let (_, by_op) = app
        .get(&format!(
            "{API}/content/{id}/restriction/byOperation/update"
        ))
        .await;
    assert_eq!(
        by_op["restrictions"]["group"]["results"][0]["name"],
        "editors"
    );
    // They are in the group, so they were not added by name.
    assert_eq!(by_op["restrictions"]["user"]["size"], 0);

    // Dev still views the page but can no longer edit, delete, or attach to it.
    let edit = json!({ "type": "page", "title": "Policy", "version": { "number": 2 }, "body": { "storage": { "value": "<p>Changed</p>", "representation": "storage" } } });
    let (status, _) = app
        .request(
            Method::GET,
            &format!("{API}/content/{id}"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = app
        .request(
            Method::PUT,
            &format!("{API}/content/{id}"),
            Some(&dev),
            Some(edit.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("{API}/content/{id}"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Nor change the restrictions themselves.
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("{API}/content/{id}/restriction"),
            Some(&dev),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Ops edits it.
    let (status, saved) = app
        .request(
            Method::PUT,
            &format!("{API}/content/{id}"),
            Some(&ops),
            Some(edit),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    // Ops cannot lock themselves out: restricting to Dev alone keeps Ops in.
    let (status, set) = app
        .request(
            Method::PUT,
            &format!("{API}/content/{id}/restriction"),
            Some(&ops),
            Some(json!({ "results": [{ "operation": "update", "restrictions": { "user": { "results": [{ "accountId": dev_account }] }, "group": [] } }] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    let users: Vec<&str> = set["results"][1]["restrictions"]["user"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["displayName"].as_str().unwrap())
        .collect();
    assert_eq!(users.len(), 2, "{set}");
    assert!(users.contains(&"Ops"));
    assert_eq!(set["results"][1]["restrictions"]["group"]["size"], 0);

    // Unknown operations are refused.
    let (status, _) = app
        .get(&format!("{API}/content/{id}/restriction/byOperation/admin"))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
