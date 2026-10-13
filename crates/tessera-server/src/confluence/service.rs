//! Rules shared by the v1 and v2 APIs: finding, creating, editing, and removing content, and
//! converting bodies between formats.

use serde_json::Value;

use super::error::{WikiError, WikiResult};
use super::render::view_html;
use crate::AppState;
use crate::store::content::{self, Content, Edit, NewContent};
use crate::store::spaces::{self, NewSpace, Space};
use crate::store::users::User;
use crate::time::now_ms;

/// An id given as a JSON string or number.
pub fn content_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|id| id.to_string()))
}

/// Content by id, in any status.
pub async fn find_content(state: &AppState, id: &str) -> WikiResult<Content> {
    let found = match id.parse::<i64>() {
        Ok(number) => content::by_id(&state.db, number).await?,
        Err(_) => None,
    };
    found.ok_or_else(|| WikiError::content_not_found(id))
}

/// Content that is current (not trashed or a draft), as most reads require.
pub async fn find_current(state: &AppState, id: &str) -> WikiResult<Content> {
    let found = find_content(state, id).await?;
    if found.status != "current" {
        return Err(WikiError::content_not_found(id));
    }
    Ok(found)
}

/// A storage-format body from a body in `representation`.
pub fn body_from(representation: &str, value: &Value) -> WikiResult<String> {
    match representation {
        "storage" | "editor" => Ok(value.as_str().unwrap_or_default().to_string()),
        "atlas_doc_format" => {
            // The ADF document arrives as a JSON string, or as an object from lenient clients.
            let doc = match value {
                Value::String(text) if text.trim().is_empty() => return Ok(String::new()),
                Value::String(text) => serde_json::from_str::<tessera_adf::Node>(text),
                other => serde_json::from_value::<tessera_adf::Node>(other.clone()),
            }
            .map_err(|e| WikiError::bad_request(format!("The ADF body is not valid: {e}")))?;
            Ok(tessera_adf::html::to_storage(&doc))
        }
        other => Err(WikiError::bad_request(format!(
            "Unsupported body representation: {other}. Use storage or atlas_doc_format."
        ))),
    }
}

/// A storage-format body in `representation`, or `None` if Tessera cannot produce it.
pub fn body_as(representation: &str, storage: &str) -> Option<String> {
    Some(match representation {
        "storage" | "editor" => storage.to_string(),
        "atlas_doc_format" => serde_json::to_string(&tessera_adf::html::from_storage(storage))
            .expect("ADF serializes"),
        "view" | "export_view" | "styled_view" | "anonymous_export_view" => view_html(storage),
        _ => return None,
    })
}

/// Creates a space with a home page named after it, as Confluence does.
pub async fn create_space(
    state: &AppState,
    user: &User,
    key: &str,
    name: &str,
    description: &str,
    space_type: &str,
) -> WikiResult<Space> {
    if key.is_empty() || key.len() > 255 || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(WikiError::bad_request(
            "Space key must only contain alphanumeric characters (a-z, 0-9)",
        ));
    }
    if name.trim().is_empty() {
        return Err(WikiError::bad_request("Space name cannot be empty"));
    }
    if spaces::by_key(&state.db, key).await?.is_some() {
        return Err(WikiError::bad_request(format!(
            "A space already exists with key {key}"
        )));
    }
    let now = now_ms();
    let space = spaces::create(
        &state.db,
        &NewSpace {
            key,
            name,
            description,
            space_type,
            creator_id: user.id,
            now,
        },
    )
    .await?;
    let home = content::create(
        &state.db,
        &NewContent {
            content_type: "page",
            status: "current",
            space_id: space.id,
            parent_id: None,
            container_id: None,
            title: &format!("{name} Home"),
            body: "",
            author_id: user.id,
            now,
        },
    )
    .await?;
    spaces::update(
        &state.db,
        space.id,
        &space.name,
        &space.description,
        &space.status,
        Some(home.id),
    )
    .await?;
    Ok(spaces::by_id(&state.db, space.id)
        .await?
        .expect("the space was just created"))
}

pub fn check_status(status: &str) -> WikiResult<()> {
    if status != "current" && status != "draft" {
        return Err(WikiError::bad_request(format!(
            "Unsupported status: {status}"
        )));
    }
    Ok(())
}

/// Titles are required, at most 255 characters, and unique among a space's pages.
pub async fn check_title(
    state: &AppState,
    space_id: i64,
    content_type: &str,
    title: &str,
    except: Option<i64>,
) -> WikiResult<()> {
    if title.trim().is_empty() {
        return Err(WikiError::bad_request("Content title cannot be empty"));
    }
    if title.chars().count() > 255 {
        return Err(WikiError::bad_request(
            "Title cannot be longer than 255 characters",
        ));
    }
    if content_type == "page"
        && content::title_taken(&state.db, space_id, content_type, title, except).await?
    {
        let key = spaces::by_id(&state.db, space_id)
            .await?
            .map(|s| s.key)
            .unwrap_or_default();
        return Err(WikiError::bad_request(format!(
            "A page with this title already exists: A page already exists with the title {title} in the space with key {key}"
        )));
    }
    Ok(())
}

/// A parent page must be a current page in the same space.
pub async fn parent_page(state: &AppState, id: &str, space_id: i64) -> WikiResult<Content> {
    let parent = find_current(state, id).await?;
    if parent.content_type != "page" || parent.space_id != space_id {
        return Err(WikiError::bad_request(format!(
            "The parent page {id} must be a page in the same space"
        )));
    }
    Ok(parent)
}

/// Refuses to move a page below itself.
pub async fn check_not_below(
    state: &AppState,
    page: &Content,
    new_parent: &Content,
) -> WikiResult<()> {
    if new_parent.id == page.id {
        return Err(WikiError::bad_request("A page cannot be its own parent"));
    }
    for ancestor in content::ancestors(&state.db, new_parent).await? {
        if ancestor.id == page.id {
            return Err(WikiError::bad_request(
                "A page cannot move below one of its own children",
            ));
        }
    }
    Ok(())
}

pub struct NewPage<'a> {
    /// `page` or `blogpost`.
    pub kind: &'a str,
    pub status: &'a str,
    pub space: &'a Space,
    pub title: &'a str,
    pub parent: Option<&'a Content>,
    pub body: &'a str,
}

pub async fn create_page(state: &AppState, user: &User, page: NewPage<'_>) -> WikiResult<Content> {
    check_status(page.status)?;
    if page.status == "current" {
        check_title(state, page.space.id, page.kind, page.title, None).await?;
    }
    Ok(content::create(
        &state.db,
        &NewContent {
            content_type: page.kind,
            status: page.status,
            space_id: page.space.id,
            parent_id: page.parent.map(|p| p.id),
            container_id: None,
            title: page.title,
            body: page.body,
            author_id: user.id,
            now: now_ms(),
        },
    )
    .await?)
}

/// Adds a comment to a page or blog post, optionally as a reply to another comment on it.
pub async fn create_comment(
    state: &AppState,
    user: &User,
    container: &Content,
    reply_to: Option<&Content>,
    body: &str,
) -> WikiResult<Content> {
    if container.content_type == "comment" {
        return Err(WikiError::bad_request(
            "A comment's container must be a page or blog post",
        ));
    }
    if let Some(parent) = reply_to
        && (parent.content_type != "comment" || parent.container_id != Some(container.id))
    {
        return Err(WikiError::bad_request(
            "A reply must answer a comment on the same content",
        ));
    }
    if body.trim().is_empty() {
        return Err(WikiError::bad_request("Comment body cannot be empty"));
    }
    Ok(content::create(
        &state.db,
        &NewContent {
            content_type: "comment",
            status: "current",
            space_id: container.space_id,
            parent_id: reply_to.map(|p| p.id),
            container_id: Some(container.id),
            title: &format!("Re: {}", container.title),
            body,
            author_id: user.id,
            now: now_ms(),
        },
    )
    .await?)
}

/// Where an edited page goes in the tree.
pub enum Parent {
    Keep,
    /// A new parent; `None` moves the page to the top of the space.
    Move(Option<Content>),
}

pub struct Save<'a> {
    /// The new version number, which must be one more than the current one.
    pub number: i64,
    pub title: Option<&'a str>,
    /// A storage-format body. `None` keeps the current body.
    pub body: Option<String>,
    pub status: &'a str,
    pub parent: Parent,
    pub message: &'a str,
    pub minor_edit: bool,
}

/// Saves a new version. A stale version number is a conflict, so two people cannot overwrite
/// each other's work.
pub async fn save(
    state: &AppState,
    user: &User,
    found: &Content,
    save: Save<'_>,
) -> WikiResult<Content> {
    if save.number != found.version + 1 {
        return Err(WikiError::conflict(format!(
            "Version must be incremented on update. Current version is: {}",
            found.version
        )));
    }
    check_status(save.status)?;
    let title = save.title.unwrap_or(&found.title).to_string();
    if found.content_type != "comment" {
        check_title(
            state,
            found.space_id,
            &found.content_type,
            &title,
            Some(found.id),
        )
        .await?;
    }
    let parent_id = match save.parent {
        Parent::Move(Some(parent)) if found.content_type == "page" => {
            check_not_below(state, found, &parent).await?;
            Some(parent.id)
        }
        Parent::Move(_) => None,
        // Restored pages return to the top of the space, as their old parent may be gone.
        Parent::Keep if found.status == "trashed" && found.content_type == "page" => None,
        Parent::Keep => found.parent_id,
    };
    let body = save.body.unwrap_or_else(|| found.body.clone());
    Ok(content::update(
        &state.db,
        found,
        &Edit {
            title: &title,
            body: &body,
            status: save.status,
            parent_id,
            author_id: user.id,
            now: now_ms(),
            message: save.message,
            minor_edit: save.minor_edit,
        },
    )
    .await?)
}

/// Moves current content to the trash, or deletes it for good when `purge` is set and it is
/// already in the trash. Comments and drafts are always deleted at once.
pub async fn remove(state: &AppState, found: &Content, purge: bool) -> WikiResult<()> {
    match found.status.as_str() {
        "current" if found.content_type == "comment" => content::purge(&state.db, found.id).await?,
        "current" => content::trash(&state.db, found, now_ms()).await?,
        "trashed" if purge => content::purge(&state.db, found.id).await?,
        "draft" => content::purge(&state.db, found.id).await?,
        _ => return Err(WikiError::content_not_found(&found.id.to_string())),
    }
    Ok(())
}
