//! Space permission checks, following Confluence.
//!
//! A space grants operations to users, groups, and anonymous visitors. Signed-in users also get
//! what anonymous visitors get. Site administrators may do everything.
//!
//! A space the user cannot read does not exist for them: reads answer 404. A user who can read
//! but lacks an operation gets 403.
//!
//! Page restrictions narrow this further: a page with view restrictions, and every page below
//! it, exists only for the people and groups it names; a page with edit restrictions can be
//! edited only by them.

use std::collections::HashSet;

use axum::http::StatusCode;
use sqlx::SqlitePool;

use super::error::{WikiError, WikiResult};
use crate::store::content::{self, Content};
use crate::store::groups;
use crate::store::restrictions::{self, Restriction};
use crate::store::space_permissions;
use crate::store::spaces;
use crate::store::users::User;

/// What one user may do in one space.
pub struct SpaceAccess {
    admin: bool,
    granted: HashSet<(String, String)>,
}

impl SpaceAccess {
    pub async fn load(db: &SqlitePool, user: &User, space_id: i64) -> sqlx::Result<SpaceAccess> {
        if user.is_admin {
            return Ok(SpaceAccess {
                admin: true,
                granted: HashSet::new(),
            });
        }
        let group_names: HashSet<String> = groups::of_user(db, user.id)
            .await?
            .into_iter()
            .map(|g| g.name.to_lowercase())
            .collect();
        let granted = space_permissions::for_space(db, space_id)
            .await?
            .into_iter()
            .filter(|p| match p.subject_type.as_str() {
                "user" => p.subject == user.account_id,
                "group" => group_names.contains(&p.subject.to_lowercase()),
                "anonymous" => true,
                _ => false,
            })
            .map(|p| (p.operation, p.target))
            .collect();
        Ok(SpaceAccess {
            admin: false,
            granted,
        })
    }

    pub fn can(&self, operation: &str, target: &str) -> bool {
        self.admin
            || self
                .granted
                .contains(&(operation.to_string(), target.to_string()))
    }

    /// Whether the user may remove content: with the operation itself, or as its author
    /// with "Delete own".
    pub fn can_delete(&self, target: &str, own: bool) -> bool {
        self.can("delete", target) || (own && self.can("delete", "space"))
    }
}

pub fn denied(operation: &str, target: &str) -> WikiError {
    WikiError::new(
        StatusCode::FORBIDDEN,
        format!("Not permitted to {operation} {target} in this space"),
    )
}

/// The user's access to a space they can read. A space they cannot read is reported missing
/// with `missing`.
pub async fn readable(
    db: &SqlitePool,
    user: &User,
    space_id: i64,
    missing: impl FnOnce() -> WikiError,
) -> WikiResult<SpaceAccess> {
    let access = SpaceAccess::load(db, user, space_id).await?;
    if !access.can("read", "space") {
        return Err(missing());
    }
    Ok(access)
}

/// Checks an operation in a space the user can read.
pub async fn require(
    db: &SqlitePool,
    user: &User,
    space_id: i64,
    operation: &str,
    target: &str,
) -> WikiResult<SpaceAccess> {
    let access = readable(db, user, space_id, || {
        WikiError::not_found("No space found")
    })
    .await?;
    if !access.can(operation, target) {
        return Err(denied(operation, target));
    }
    Ok(access)
}

/// Ids of the spaces a user can read, or `None` when they can read every space.
pub async fn readable_spaces(db: &SqlitePool, user: &User) -> WikiResult<Option<Vec<i64>>> {
    if user.is_admin {
        return Ok(None);
    }
    let filter = spaces::SpaceFilter::default();
    let (all, _) = spaces::list(db, &filter, 0, i64::MAX / 2).await?;
    let mut ids = Vec::new();
    let mut every = true;
    for space in &all {
        if SpaceAccess::load(db, user, space.id)
            .await?
            .can("read", "space")
        {
            ids.push(space.id);
        } else {
            every = false;
        }
    }
    Ok(if every { None } else { Some(ids) })
}

/// Keeps the content in spaces the user can read.
pub async fn retain_readable(
    db: &SqlitePool,
    user: &User,
    items: &mut Vec<Content>,
) -> WikiResult<()> {
    if let Some(ids) = readable_spaces(db, user).await? {
        items.retain(|c| ids.contains(&c.space_id));
    }
    Ok(())
}

/// Who the user is, for page restrictions: their account and their groups.
struct Viewer {
    admin: bool,
    account_id: String,
    groups: HashSet<String>,
}

impl Viewer {
    async fn load(db: &SqlitePool, user: &User) -> sqlx::Result<Viewer> {
        Ok(Viewer {
            admin: user.is_admin,
            account_id: user.account_id.clone(),
            groups: groups::of_user(db, user.id)
                .await?
                .into_iter()
                .map(|g| g.name.to_lowercase())
                .collect(),
        })
    }

    fn named_in(&self, restriction: &Restriction) -> bool {
        match restriction.subject_type.as_str() {
            "user" => restriction.subject == self.account_id,
            "group" => self.groups.contains(&restriction.subject.to_lowercase()),
            _ => false,
        }
    }

    /// Whether the user passes the restrictions of one operation on one piece of content.
    async fn passes(
        &self,
        db: &SqlitePool,
        content_id: i64,
        operation: &str,
    ) -> sqlx::Result<bool> {
        let named: Vec<Restriction> = restrictions::of_content(db, content_id)
            .await?
            .into_iter()
            .filter(|r| r.operation == operation)
            .collect();
        Ok(named.is_empty() || named.iter().any(|r| self.named_in(r)))
    }
}

/// The page whose restrictions govern content: a comment's or attachment's page, or itself.
async fn governing_page(db: &SqlitePool, item: &Content) -> sqlx::Result<Content> {
    match (item.content_type.as_str(), item.container_id) {
        ("comment" | "attachment", Some(container)) => Ok(content::by_id(db, container)
            .await?
            .unwrap_or_else(|| item.clone())),
        _ => Ok(item.clone()),
    }
}

/// Whether page restrictions let the user view content. A view restriction on a page also
/// hides every page below it.
pub async fn can_view(db: &SqlitePool, user: &User, item: &Content) -> sqlx::Result<bool> {
    let viewer = Viewer::load(db, user).await?;
    if viewer.admin {
        return Ok(true);
    }
    let page = governing_page(db, item).await?;
    for node in content::ancestors(db, &page)
        .await?
        .iter()
        .chain(std::iter::once(&page))
    {
        if !viewer.passes(db, node.id, "read").await? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether page restrictions let the user edit content (or attach files to it). Edit
/// restrictions apply to the page itself only.
pub async fn can_edit(db: &SqlitePool, user: &User, item: &Content) -> sqlx::Result<bool> {
    let viewer = Viewer::load(db, user).await?;
    if viewer.admin {
        return Ok(true);
    }
    let page = governing_page(db, item).await?;
    Ok(viewer.passes(db, page.id, "update").await? && can_view(db, user, item).await?)
}

/// Keeps the content the user may see: in spaces they can read, and not hidden from them by
/// page restrictions.
pub async fn retain_visible(
    db: &SqlitePool,
    user: &User,
    items: &mut Vec<Content>,
) -> WikiResult<()> {
    retain_readable(db, user, items).await?;
    let mut kept = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        if can_view(db, user, &item).await? {
            kept.push(item);
        }
    }
    *items = kept;
    Ok(())
}
