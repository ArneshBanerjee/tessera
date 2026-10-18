//! JSON for the Confluence v1 REST API, with its `expand` rules: nested objects appear only
//! when asked for, and the rest are listed under `_expandable`.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use super::access;
use super::error::WikiResult;
use super::service::{self, Names};
use crate::AppState;
use crate::store::content::{self, Content, Version};
use crate::store::labels::{self, Label};
use crate::store::space_permissions::{self, SpacePermission};
use crate::store::spaces::{self, Space};
use crate::store::users::{self, User};
use crate::store::wiki_files;
use crate::time::format_iso;

/// The `expand` parameter: dotted paths such as `body.storage` or `children.page`.
#[derive(Debug, Clone, Default)]
pub struct Expand(Vec<String>);

impl Expand {
    pub fn parse(text: Option<&str>) -> Self {
        Expand(
            text.unwrap_or_default()
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
        )
    }

    /// Whether `path`, or anything below it, was asked for.
    pub fn has(&self, path: &str) -> bool {
        self.0.iter().any(|p| {
            p == path || (p.starts_with(path) && p.as_bytes().get(path.len()) == Some(&b'.'))
        })
    }

    /// The paths below `prefix`, for rendering a nested object.
    pub fn below(&self, prefix: &str) -> Expand {
        let start = format!("{prefix}.");
        Expand(
            self.0
                .iter()
                .filter_map(|p| p.strip_prefix(&start).map(str::to_string))
                .collect(),
        )
    }

    pub fn with(mut self, path: &str) -> Self {
        self.0.push(path.to_string());
        self
    }
}

/// Body formats a client may ask for.
pub const REPRESENTATIONS: &[&str] = &[
    "storage",
    "atlas_doc_format",
    "view",
    "export_view",
    "styled_view",
    "editor",
    "anonymous_export_view",
];

/// Shared state for rendering one response: URLs and a cache of loaded users.
pub struct WikiCtx<'a> {
    pub state: &'a AppState,
    /// Who the response is for. Expanded children leave out what they may not see.
    viewer: &'a User,
    users: HashMap<i64, Option<User>>,
    spaces: HashMap<i64, Option<Space>>,
    /// Loaded the first time a body is drawn for display.
    names: Option<Names>,
}

impl<'a> WikiCtx<'a> {
    pub fn new(state: &'a AppState, viewer: &'a User) -> Self {
        Self {
            state,
            viewer,
            users: HashMap::new(),
            spaces: HashMap::new(),
            names: None,
        }
    }

    async fn names(&mut self) -> WikiResult<&Names> {
        if self.names.is_none() {
            self.names = Some(service::load_names(&self.state.db).await?);
        }
        Ok(self.names.as_ref().expect("names were just loaded"))
    }

    /// The base of Confluence URLs, such as `https://example.com/wiki`.
    pub fn base(&self) -> String {
        format!("{}/wiki", self.state.base_url)
    }

    pub fn api_url(&self, path: &str) -> String {
        format!("{}/rest/api{path}", self.base())
    }

    /// The `_links` every collection carries.
    pub fn collection(
        &self,
        results: Vec<Value>,
        start: i64,
        limit: i64,
        path: &str,
        more: bool,
    ) -> Value {
        let mut links = json!({
            "base": self.base(),
            "context": "/wiki",
            "self": self.api_url(path),
        });
        if more {
            let separator = if path.contains('?') { '&' } else { '?' };
            links["next"] = json!(format!(
                "/rest/api{path}{separator}start={}&limit={limit}",
                start + limit
            ));
        }
        json!({
            "results": results,
            "start": start,
            "limit": limit,
            "size": results.len(),
            "_links": links,
        })
    }

    pub async fn load_user(&mut self, id: i64) -> WikiResult<Option<User>> {
        if !self.users.contains_key(&id) {
            let user = users::by_id(&self.state.db, id).await?;
            self.users.insert(id, user);
        }
        Ok(self.users[&id].clone())
    }

    async fn load_space(&mut self, id: i64) -> WikiResult<Option<Space>> {
        if !self.spaces.contains_key(&id) {
            let space = spaces::by_id(&self.state.db, id).await?;
            self.spaces.insert(id, space);
        }
        Ok(self.spaces[&id].clone())
    }

    /// A user the way Confluence shows them.
    pub fn user(&self, user: &User) -> Value {
        json!({
            "type": "known",
            "accountId": user.account_id,
            "accountType": "atlassian",
            "email": user.email,
            "publicName": user.display_name,
            "displayName": user.display_name,
            "timeZone": user.time_zone,
            "isExternalCollaborator": false,
            "profilePicture": {
                "path": format!("/avatar/user/{}?size=48", user.account_id),
                "width": 48,
                "height": 48,
                "isDefault": false,
            },
            "_links": { "self": self.api_url(&format!("/user?accountId={}", user.account_id)) },
        })
    }

    pub async fn user_by_id(&mut self, id: i64) -> WikiResult<Value> {
        Ok(match self.load_user(id).await? {
            Some(user) => self.user(&user),
            None => json!({ "type": "anonymous", "displayName": "Anonymous" }),
        })
    }

    pub async fn space(&mut self, space: &Space, expand: &Expand) -> WikiResult<Value> {
        let mut out = json!({
            "id": space.id,
            "key": space.key,
            "name": space.name,
            "type": space.space_type,
            "status": space.status,
            "_links": {
                "webui": format!("/spaces/{}", space.key),
                "self": self.api_url(&format!("/space/{}", space.key)),
            },
        });
        let mut expandable = Map::new();
        if expand.has("description") {
            out["description"] = json!({
                "plain": { "value": space.description, "representation": "plain", "embeddedContent": [] },
            });
        } else {
            expandable.insert("description".into(), json!(""));
        }
        match space.homepage_id {
            Some(id) if expand.has("homepage") => {
                if let Some(home) = content::by_id(&self.state.db, id).await? {
                    out["homepage"] =
                        Box::pin(self.content(&home, &expand.below("homepage"))).await?;
                }
            }
            Some(id) => {
                expandable.insert("homepage".into(), json!(format!("/rest/api/content/{id}")));
            }
            None => {}
        }
        if expand.has("permissions") {
            let mut list = Vec::new();
            for permission in space_permissions::for_space(&self.state.db, space.id).await? {
                list.push(self.space_permission(&permission).await?);
            }
            out["permissions"] = json!(list);
        } else {
            expandable.insert("permissions".into(), json!(""));
        }
        for key in [
            "icon",
            "metadata",
            "operations",
            "settings",
            "theme",
            "lookAndFeel",
            "history",
        ] {
            expandable.insert(key.into(), json!(""));
        }
        out["_expandable"] = Value::Object(expandable);
        Ok(out)
    }

    /// A space permission in the v1 shape: who it is for, and which operation it grants.
    pub async fn space_permission(&mut self, permission: &SpacePermission) -> WikiResult<Value> {
        let mut subjects = Value::Null;
        match permission.subject_type.as_str() {
            "user" => {
                if let Some(user) =
                    users::by_account_id(&self.state.db, &permission.subject).await?
                {
                    subjects = json!({ "user": { "results": [self.user(&user)], "size": 1 } });
                }
            }
            "group" => {
                subjects = json!({ "group": { "results": [{ "type": "group", "name": permission.subject }], "size": 1 } });
            }
            _ => {}
        }
        Ok(json!({
            "id": permission.id,
            "subjects": subjects,
            "operation": { "operation": permission.operation, "targetType": permission.target },
            "anonymousAccess": permission.subject_type == "anonymous",
            "unlicensedAccess": false,
        }))
    }

    async fn space_by_id(&mut self, id: i64, expand: &Expand) -> WikiResult<Value> {
        match self.load_space(id).await? {
            Some(space) => self.space(&space, expand).await,
            None => Ok(Value::Null),
        }
    }

    pub async fn version(&mut self, version: &Version, content: &Content) -> WikiResult<Value> {
        Ok(json!({
            "by": self.user_by_id(version.author_id).await?,
            "when": format_iso(version.created_at),
            "friendlyWhen": format_iso(version.created_at),
            "message": version.message,
            "number": version.number,
            "minorEdit": version.minor_edit,
            "contentTypeModified": false,
            "_links": { "self": self.api_url(&format!("/content/{}/version/{}", content.id, version.number)) },
        }))
    }

    /// The `version` object of the current version, from the content row.
    async fn current_version(&mut self, content: &Content) -> WikiResult<Value> {
        match content::version(&self.state.db, content.id, content.version).await? {
            Some(version) => self.version(&version, content).await,
            None => Ok(Value::Null),
        }
    }

    pub async fn history(&mut self, content: &Content, expand: &Expand) -> WikiResult<Value> {
        let mut out = json!({
            "latest": true,
            "createdBy": self.user_by_id(content.creator_id).await?,
            "createdDate": format_iso(content.created_at),
            "_links": { "self": self.api_url(&format!("/content/{}/history", content.id)) },
        });
        let mut expandable = Map::new();
        if expand.has("lastUpdated") {
            out["lastUpdated"] = self.current_version(content).await?;
        } else {
            expandable.insert("lastUpdated".into(), json!(""));
        }
        if expand.has("previousVersion") && content.version > 1 {
            if let Some(previous) =
                content::version(&self.state.db, content.id, content.version - 1).await?
            {
                out["previousVersion"] = self.version(&previous, content).await?;
            }
        } else {
            expandable.insert("previousVersion".into(), json!(""));
        }
        if expand.has("contributors") {
            let mut publishers = Vec::new();
            for id in content::contributors(&self.state.db, content.id).await? {
                publishers.push(self.user_by_id(id).await?);
            }
            out["contributors"] = json!({ "publishers": { "users": publishers } });
        } else {
            expandable.insert("contributors".into(), json!(""));
        }
        expandable.insert("nextVersion".into(), json!(""));
        out["_expandable"] = Value::Object(expandable);
        Ok(out)
    }

    pub fn label(&self, label: &Label) -> Value {
        json!({
            "prefix": label.prefix,
            "name": label.name,
            "id": label.id.to_string(),
            "label": label.name,
        })
    }

    pub async fn labels(&mut self, content_id: i64) -> WikiResult<Value> {
        let list = labels::for_content(&self.state.db, content_id).await?;
        let results: Vec<Value> = list.iter().map(|l| self.label(l)).collect();
        let count = results.len() as i64;
        Ok(self.collection(
            results,
            0,
            count.max(200),
            &format!("/content/{content_id}/label"),
            false,
        ))
    }

    /// The web UI path of content, which Confluence clients use to build links.
    fn webui(&self, content: &Content, space_key: &str) -> String {
        let title = form_urlencoded::byte_serialize(content.title.as_bytes()).collect::<String>();
        match content.content_type.as_str() {
            "blogpost" => format!("/spaces/{space_key}/blog/{}/{title}", content.id),
            "comment" => match content.container_id {
                Some(container) => format!(
                    "/spaces/{space_key}/pages/{container}?focusedCommentId={}#comment-{}",
                    content.id, content.id
                ),
                None => format!("/spaces/{space_key}"),
            },
            _ => format!("/spaces/{space_key}/pages/{}/{title}", content.id),
        }
    }

    /// Content as JSON. `expand` picks the nested objects to include.
    pub async fn content(&mut self, content: &Content, expand: &Expand) -> WikiResult<Value> {
        let space = self.load_space(content.space_id).await?;
        let space_key = space.as_ref().map(|s| s.key.clone()).unwrap_or_default();
        let id = content.id;
        let mut out = json!({
            "id": content_id_text(content),
            "type": content.content_type,
            "status": content.status,
            "title": content.title,
            "macroRenderedOutput": {},
            "extensions": if content.content_type == "page" {
                json!({ "position": content.position })
            } else {
                json!({})
            },
            "_links": {
                "webui": self.webui(content, &space_key),
                "edit": format!("/pages/resumedraft.action?draftId={id}"),
                "tinyui": format!("/x/{}", tiny_id(id)),
                "self": self.api_url(&format!("/content/{id}")),
            },
        });
        if content.content_type == "attachment" {
            self.attachment_fields(content, &mut out).await?;
        }
        let mut expandable = Map::new();
        let mut expandable_link = |key: &str, path: String| {
            expandable.insert(key.into(), json!(path));
        };

        if expand.has("space") {
            out["space"] = self
                .space_by_id(content.space_id, &expand.below("space"))
                .await?;
        } else {
            expandable_link("space", format!("/rest/api/space/{space_key}"));
        }
        if expand.has("history") {
            out["history"] = self.history(content, &expand.below("history")).await?;
        } else {
            expandable_link("history", format!("/rest/api/content/{id}/history"));
        }
        if expand.has("version") {
            out["version"] = self.current_version(content).await?;
        } else {
            expandable_link("version", String::new());
        }
        if expand.has("ancestors") {
            let mut list = Vec::new();
            for ancestor in content::ancestors(&self.state.db, content).await? {
                list.push(Box::pin(self.content(&ancestor, &expand.below("ancestors"))).await?);
            }
            out["ancestors"] = json!(list);
        } else {
            expandable_link("ancestors", String::new());
        }
        if expand.has("container") {
            out["container"] = match content.container_id {
                Some(container) => match content::by_id(&self.state.db, container).await? {
                    Some(parent) => {
                        Box::pin(self.content(&parent, &expand.below("container"))).await?
                    }
                    None => Value::Null,
                },
                None => {
                    self.space_by_id(content.space_id, &expand.below("container"))
                        .await?
                }
            };
        } else {
            expandable_link("container", format!("/rest/api/space/{space_key}"));
        }
        if expand.has("body") {
            out["body"] = self.body(content, &expand.below("body")).await?;
        } else {
            expandable_link("body", String::new());
        }
        if expand.has("children") {
            out["children"] = Box::pin(self.children(content, &expand.below("children"))).await?;
        } else {
            expandable_link("children", format!("/rest/api/content/{id}/child"));
        }
        if expand.has("descendants") {
            out["descendants"] =
                Box::pin(self.descendants(content, &expand.below("descendants"))).await?;
        } else {
            expandable_link("descendants", format!("/rest/api/content/{id}/descendant"));
        }
        if expand.has("metadata") {
            let metadata = expand.below("metadata");
            let mut value =
                json!({ "_expandable": { "currentuser": "", "properties": "", "frontend": "" } });
            if metadata.has("labels") {
                value["labels"] = self.labels(id).await?;
            } else {
                value["_expandable"]["labels"] = json!("");
            }
            out["metadata"] = value;
        } else {
            expandable_link("metadata", String::new());
        }
        if expand.has("restrictions")
            && matches!(content.content_type.as_str(), "page" | "blogpost")
        {
            out["restrictions"] = super::restrictions::by_operation_json(self, content).await?;
        } else {
            expandable_link(
                "restrictions",
                format!("/rest/api/content/{id}/restriction/byOperation"),
            );
        }
        for key in [
            "childTypes",
            "operations",
            "schedulePublishDate",
            "schedulePublishInfo",
        ] {
            expandable_link(key, String::new());
        }
        out["_expandable"] = Value::Object(expandable);
        Ok(out)
    }

    async fn body(&mut self, content: &Content, expand: &Expand) -> WikiResult<Value> {
        let mut out = Map::new();
        let mut expandable = Map::new();
        for representation in REPRESENTATIONS {
            // Convert only the formats the client asked for.
            let value = if expand.has(representation) {
                let view = service::View {
                    names: self.names().await?,
                    page_id: Some(images_page(content)),
                };
                service::body_as(representation, &content.body, &view)
            } else {
                None
            };
            match value {
                Some(value) => {
                    out.insert(
                        (*representation).into(),
                        json!({
                            "value": value,
                            "representation": representation,
                            "embeddedContent": [],
                            "_expandable": { "content": format!("/rest/api/content/{}", content.id) },
                        }),
                    );
                }
                None => {
                    expandable.insert((*representation).into(), json!(""));
                }
            }
        }
        expandable.insert("dynamic".into(), json!(""));
        out.insert("_expandable".into(), Value::Object(expandable));
        Ok(Value::Object(out))
    }

    /// What attachments add to content: the file's type, size, and comment, and its download link.
    async fn attachment_fields(&mut self, attachment: &Content, out: &mut Value) -> WikiResult<()> {
        let file =
            wiki_files::find(&self.state.db, attachment.id, Some(attachment.version)).await?;
        let (media_type, size, comment) = file
            .map(|f| (f.media_type, f.size, f.comment))
            .unwrap_or_default();
        let container = attachment.container_id.unwrap_or_default();
        let name = form_urlencoded::byte_serialize(attachment.title.as_bytes()).collect::<String>();
        let download = service::download_path(container, &attachment.title);
        out["metadata"] = json!({
            "mediaType": media_type,
            "comment": comment,
            "_expandable": { "currentuser": "", "properties": "", "frontend": "", "labels": "" },
        });
        out["extensions"] =
            json!({ "mediaType": media_type, "fileSize": size, "comment": comment });
        out["_links"]["download"] = json!(format!(
            "{download}?version={}&modificationDate={}&api=v2",
            attachment.version, attachment.updated_at
        ));
        out["_links"]["webui"] = json!(format!(
            "/pages/viewpageattachments.action?pageId={container}&preview=%2F{container}%2F{name}"
        ));
        Ok(())
    }

    /// Children of each requested type: `page`, `comment`, and `attachment`.
    pub async fn children(&mut self, content: &Content, expand: &Expand) -> WikiResult<Value> {
        let mut out = Map::new();
        let mut expandable = Map::new();
        for kind in ["page", "comment", "attachment"] {
            if expand.has(kind) {
                let list = child_list(&self.state.db, self.viewer, content, kind).await?;
                let mut results = Vec::with_capacity(list.len());
                for child in &list {
                    results.push(Box::pin(self.content(child, &expand.below(kind))).await?);
                }
                let path = format!("/content/{}/child/{kind}", content.id);
                out.insert(
                    kind.into(),
                    self.collection(results, 0, 25.max(list.len() as i64), &path, false),
                );
            } else {
                expandable.insert(
                    kind.into(),
                    json!(format!("/rest/api/content/{}/child/{kind}", content.id)),
                );
            }
        }
        out.insert("_expandable".into(), Value::Object(expandable));
        out.insert(
            "_links".into(),
            json!({ "self": self.api_url(&format!("/content/{}/child", content.id)) }),
        );
        Ok(Value::Object(out))
    }

    async fn descendants(&mut self, content: &Content, expand: &Expand) -> WikiResult<Value> {
        let mut out = Map::new();
        for kind in ["page", "comment"] {
            if expand.has(kind) {
                let list = descendant_list(&self.state.db, self.viewer, content, kind).await?;
                let mut results = Vec::with_capacity(list.len());
                for child in &list {
                    results.push(Box::pin(self.content(child, &expand.below(kind))).await?);
                }
                let path = format!("/content/{}/descendant/{kind}", content.id);
                out.insert(
                    kind.into(),
                    self.collection(results, 0, 25.max(list.len() as i64), &path, false),
                );
            }
        }
        Ok(Value::Object(out))
    }
}

/// Direct children of one type that the user may see. Comments are the top-level comments on a
/// page.
pub async fn child_list(
    db: &sqlx::SqlitePool,
    user: &User,
    content: &Content,
    kind: &str,
) -> WikiResult<Vec<Content>> {
    let mut list = match kind {
        "page" if content.content_type == "page" => {
            content::children(db, content.id, "page").await?
        }
        "attachment" => content::attachments(db, content.id).await?,
        "comment" => content::comments(db, content.id)
            .await?
            .into_iter()
            .filter(|c| c.parent_id.is_none())
            .collect(),
        _ => Vec::new(),
    };
    access::retain_visible(db, user, &mut list).await?;
    Ok(list)
}

/// Everything of one type below content that the user may see: all pages under a page, or all
/// comments on it.
pub async fn descendant_list(
    db: &sqlx::SqlitePool,
    user: &User,
    content: &Content,
    kind: &str,
) -> WikiResult<Vec<Content>> {
    let mut list = match kind {
        "page" if content.content_type == "page" => {
            content::descendants(db, content.id, "page").await?
        }
        "comment" => content::comments(db, content.id).await?,
        _ => Vec::new(),
    };
    access::retain_visible(db, user, &mut list).await?;
    Ok(list)
}

/// The page whose attachments a body's images name: a comment's page, or the content itself.
pub fn images_page(content: &Content) -> i64 {
    match content.content_type.as_str() {
        "comment" | "attachment" => content.container_id.unwrap_or(content.id),
        _ => content.id,
    }
}

/// Content ids as the API shows them. Attachments carry an `att` prefix, as in Confluence.
pub fn content_id_text(content: &Content) -> String {
    if content.content_type == "attachment" {
        format!("att{}", content.id)
    } else {
        content.id.to_string()
    }
}

/// Confluence's short link id: the content id's little-endian bytes in URL-safe base64.
fn tiny_id(id: i64) -> String {
    use base64::Engine;
    let bytes = id.to_le_bytes();
    let used = bytes.iter().rposition(|b| *b != 0).map_or(1, |i| i + 1);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes[..used])
}
