//! Tessera's account API: API tokens, passwords, profiles, and user administration.

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, basic};
use serde_json::json;

const ACCOUNT: &str = "/rest/tessera/1.0";

#[tokio::test]
async fn api_tokens_work_until_revoked() {
    let app = TestApp::new().await;
    let (user, _) = app.user("dev@tessera.test", "Dev").await;
    let (status, created) = app
        .request(
            Method::POST,
            &format!("{ACCOUNT}/api-tokens"),
            Some(&user),
            Some(json!({ "label": "CI" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = created["token"].as_str().unwrap().to_string();
    let token_auth = basic("dev@tessera.test", &token);

    let (status, me) = app
        .request(Method::GET, "/rest/api/3/myself", Some(&token_auth), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["displayName"], "Dev");

    let (_, list) = app
        .request(
            Method::GET,
            &format!("{ACCOUNT}/api-tokens"),
            Some(&user),
            None,
        )
        .await;
    assert_eq!(list[0]["label"], "CI");
    assert!(
        list[0]["lastUsed"].is_string(),
        "using the token records it"
    );
    assert!(list[0].get("token").is_none(), "secrets are never listed");

    let id = created["id"].as_str().unwrap();
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("{ACCOUNT}/api-tokens/{id}"),
            Some(&user),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .request(Method::GET, "/rest/api/3/myself", Some(&token_auth), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Nobody can revoke another user's token.
    let (status, again) = app
        .request(
            Method::POST,
            &format!("{ACCOUNT}/api-tokens"),
            Some(&user),
            Some(json!({ "label": "x" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = again["id"].as_str().unwrap();
    let (status, _) = app.delete(&format!("{ACCOUNT}/api-tokens/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn passwords_and_profiles() {
    let app = TestApp::new().await;
    let (user, _) = app.user("dev@tessera.test", "Dev").await;
    let path = format!("{ACCOUNT}/password");
    let (status, error) = app
        .request(
            Method::PUT,
            &path,
            Some(&user),
            Some(json!({ "currentPassword": "wrong", "newPassword": "longenough" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error["errors"]["currentPassword"].is_string());
    let (status, _) = app
        .request(
            Method::PUT,
            &path,
            Some(&user),
            Some(json!({ "currentPassword": "pw", "newPassword": "short" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = app
        .request(
            Method::PUT,
            &path,
            Some(&user),
            Some(json!({ "currentPassword": "pw", "newPassword": "longenough" })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let new_auth = basic("dev@tessera.test", "longenough");
    let (status, _) = app
        .request(Method::GET, "/rest/api/3/myself", Some(&user), None)
        .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the old password stops working"
    );

    let (status, profile) = app
        .request(
            Method::PUT,
            &format!("{ACCOUNT}/profile"),
            Some(&new_auth),
            Some(json!({ "displayName": "Dev Person", "timeZone": "Europe/Berlin" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(profile["displayName"], "Dev Person");
    let (status, _) = app
        .request(
            Method::PUT,
            &format!("{ACCOUNT}/profile"),
            Some(&new_auth),
            Some(json!({ "timeZone": "Mars/Base" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn administrators_manage_users() {
    let app = TestApp::new().await;
    let (user, _) = app.user("dev@tessera.test", "Dev").await;
    let users = format!("{ACCOUNT}/admin/users");

    let (status, _) = app.request(Method::GET, &users, Some(&user), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, created) = app
        .post(&users, json!({ "emailAddress": "new@tessera.test", "displayName": "New", "password": "longenough" }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let account = created["accountId"].as_str().unwrap().to_string();
    let new_auth = basic("new@tessera.test", "longenough");
    let (status, _) = app
        .request(Method::GET, "/rest/api/3/myself", Some(&new_auth), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    // Deactivated users cannot sign in.
    let (status, _) = app
        .put(&format!("{users}/{account}"), json!({ "active": false }))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = app
        .request(Method::GET, "/rest/api/3/myself", Some(&new_auth), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The last administrator cannot step down.
    let (_, me) = app.get("/rest/api/3/myself").await;
    let admin_account = me["accountId"].as_str().unwrap();
    let (status, error) = app
        .put(
            &format!("{users}/{admin_account}"),
            json!({ "admin": false }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");

    let (_, list) = app.get(&users).await;
    assert_eq!(list.as_array().unwrap().len(), 3);
}
