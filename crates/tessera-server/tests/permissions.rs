//! Permission schemes, project roles, and groups decide who sees and changes what.

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::{Value, json};

const API: &str = "/rest/api/3";

/// The admin's project PROJ with one issue, and a regular user's auth header and account id.
async fn setup() -> (TestApp, String, String, String) {
    let app = TestApp::new().await;
    app.project("PROJ").await;
    let key = app.issue("PROJ", "First", json!({})).await;
    let (dev, account) = app.user("dev@tessera.test", "Dev").await;
    (app, key, dev, account)
}

async fn as_user(
    app: &TestApp,
    auth: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    app.request(method, path, Some(auth), body).await
}

#[tokio::test]
async fn the_default_scheme_lets_users_work_but_not_delete() {
    let (app, key, dev, _) = setup().await;
    let (status, _) = as_user(&app, &dev, Method::GET, &format!("{API}/issue/{key}"), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, created) = as_user(
        &app,
        &dev,
        Method::POST,
        &format!("{API}/issue"),
        Some(json!({ "fields": { "project": { "key": "PROJ" }, "issuetype": { "name": "Task" }, "summary": "Mine" } })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, _) = as_user(
        &app,
        &dev,
        Method::PUT,
        &format!("{API}/issue/{key}"),
        Some(json!({ "fields": { "summary": "Edited" } })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Deleting needs the project's Administrators role.
    let (status, error) = as_user(
        &app,
        &dev,
        Method::DELETE,
        &format!("{API}/issue/{key}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        error["errorMessages"][0],
        "You do not have permission to delete issues in this project."
    );

    // Project settings too.
    let (status, _) = as_user(
        &app,
        &dev,
        Method::PUT,
        &format!("{API}/project/PROJ"),
        Some(json!({ "name": "Renamed" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (_, mine) = as_user(
        &app,
        &dev,
        Method::GET,
        &format!("{API}/mypermissions?projectKey=PROJ&permissions=BROWSE_PROJECTS,DELETE_ISSUES"),
        None,
    )
    .await;
    assert_eq!(
        mine["permissions"]["BROWSE_PROJECTS"]["havePermission"],
        true
    );
    assert_eq!(
        mine["permissions"]["DELETE_ISSUES"]["havePermission"],
        false
    );
}

#[tokio::test]
async fn project_roles_grant_administration() {
    let (app, key, dev, account) = setup().await;
    // The creator of PROJ leads it, so they are in its Administrators role.
    let (_, admins) = app.get(&format!("{API}/project/PROJ/role/10002")).await;
    assert_eq!(admins["actors"].as_array().unwrap().len(), 1);

    let (status, role) = app
        .post(
            &format!("{API}/project/PROJ/role/10002"),
            json!({ "user": [account] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{role}");
    assert_eq!(role["actors"].as_array().unwrap().len(), 2);

    let (status, _) = as_user(
        &app,
        &dev,
        Method::DELETE,
        &format!("{API}/issue/{key}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = app
        .delete(&format!("{API}/project/PROJ/role/10002?user={account}"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = as_user(
        &app,
        &dev,
        Method::PUT,
        &format!("{API}/project/PROJ"),
        Some(json!({ "name": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_private_scheme_hides_the_project() {
    let (app, key, dev, account) = setup().await;
    let (status, scheme) = app
        .post(
            &format!("{API}/permissionscheme"),
            json!({
                "name": "Members only",
                "permissions": [
                    { "holder": { "type": "projectRole", "parameter": "10003" }, "permission": "BROWSE_PROJECTS" },
                    { "holder": { "type": "projectRole", "parameter": "10003" }, "permission": "CREATE_ISSUES" },
                ],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{scheme}");
    assert_eq!(scheme["permissions"].as_array().unwrap().len(), 2);
    let scheme_id = scheme["id"].as_i64().unwrap();
    let (status, _) = app
        .put(
            &format!("{API}/project/PROJ/permissionscheme"),
            json!({ "id": scheme_id }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // To an outsider the project and its issues do not exist.
    let (status, _) = as_user(&app, &dev, Method::GET, &format!("{API}/issue/{key}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = as_user(
        &app,
        &dev,
        Method::GET,
        &format!("{API}/project/PROJ"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, list) = as_user(&app, &dev, Method::GET, &format!("{API}/project"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 0);
    let (_, found) = as_user(
        &app,
        &dev,
        Method::POST,
        &format!("{API}/search"),
        Some(json!({ "jql": "project = PROJ" })),
    )
    .await;
    assert_eq!(found["total"], 0);
    let (_, found) = as_user(
        &app,
        &dev,
        Method::POST,
        &format!("{API}/search"),
        Some(json!({ "jql": "" })),
    )
    .await;
    assert_eq!(found["total"], 0);

    // Through a group in the Members role, they see it.
    let (status, _) = app
        .post(&format!("{API}/group"), json!({ "name": "developers" }))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = app
        .post(
            &format!("{API}/group/user?groupname=developers"),
            json!({ "accountId": account }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = app
        .post(
            &format!("{API}/project/PROJ/role/10003"),
            json!({ "group": ["developers"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = as_user(&app, &dev, Method::GET, &format!("{API}/issue/{key}"), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, found) = as_user(
        &app,
        &dev,
        Method::POST,
        &format!("{API}/search"),
        Some(json!({ "jql": "project = PROJ" })),
    )
    .await;
    assert_eq!(found["total"], 1);
    // The scheme does not grant editing.
    let (status, _) = as_user(
        &app,
        &dev,
        Method::PUT,
        &format!("{API}/issue/{key}"),
        Some(json!({ "fields": { "summary": "Edited" } })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (_, groups) = as_user(
        &app,
        &dev,
        Method::GET,
        &format!("{API}/user/groups?accountId={account}"),
        None,
    )
    .await;
    assert_eq!(groups[0]["name"], "developers");

    // Deleting the scheme returns the project to the default.
    let (status, _) = app
        .delete(&format!("{API}/permissionscheme/{scheme_id}"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, current) = app
        .get(&format!("{API}/project/PROJ/permissionscheme"))
        .await;
    assert_eq!(current["id"], 0);
}

#[tokio::test]
async fn others_comments_need_edit_all() {
    let (app, key, dev, _) = setup().await;
    let (_, comment) = app
        .post(
            &format!("{API}/issue/{key}/comment"),
            json!({ "body": { "type": "doc", "version": 1, "content": [] } }),
        )
        .await;
    let id = comment["id"].as_str().unwrap();
    let (status, _) = as_user(
        &app,
        &dev,
        Method::DELETE,
        &format!("{API}/issue/{key}/comment/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = app.delete(&format!("{API}/issue/{key}/comment/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}
