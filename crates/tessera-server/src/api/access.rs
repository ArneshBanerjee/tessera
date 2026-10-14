//! Permission checks, following Jira's permission schemes.
//!
//! A project uses one permission scheme. The scheme grants each permission to holders: anyone,
//! every signed-in user (`applicationRole`), a group, a user, a project role, the project lead,
//! or an issue's reporter or assignee. A user has a permission when they are one of its holders.
//! Site administrators have every permission.
//!
//! Projects a user cannot browse do not exist for them: reads answer 404, as Jira does. A user
//! who can browse but lacks the permission for an action gets 403.

use std::collections::HashSet;

use sqlx::SqlitePool;

use crate::AppState;
use crate::error::{ApiError, ApiResult};
use crate::store::groups;
use crate::store::issues::{self, Issue};
use crate::store::permissions::{self, Grant};
use crate::store::projects::{self, Project};
use crate::store::users::User;

/// Every project permission: key, name, and what it allows.
pub const PROJECT_PERMISSIONS: &[(&str, &str, &str)] = &[
    (
        "ADMINISTER_PROJECTS",
        "Administer Projects",
        "Ability to administer a project in Jira.",
    ),
    (
        "BROWSE_PROJECTS",
        "Browse Projects",
        "Ability to browse projects and the issues within them.",
    ),
    (
        "MANAGE_SPRINTS_PERMISSION",
        "Manage sprints",
        "Ability to manage sprints.",
    ),
    (
        "VIEW_READONLY_WORKFLOW",
        "View Read-Only Workflow",
        "Users with this permission may view a read-only version of a workflow.",
    ),
    (
        "CREATE_ISSUES",
        "Create Issues",
        "Ability to create issues.",
    ),
    ("EDIT_ISSUES", "Edit Issues", "Ability to edit issues."),
    (
        "TRANSITION_ISSUES",
        "Transition Issues",
        "Ability to transition issues.",
    ),
    (
        "SCHEDULE_ISSUES",
        "Schedule Issues",
        "Ability to view or edit an issue's due date.",
    ),
    (
        "MOVE_ISSUES",
        "Move Issues",
        "Ability to move issues between projects or between workflows of the same project (if applicable).",
    ),
    (
        "ASSIGN_ISSUES",
        "Assign Issues",
        "Ability to assign issues to other people.",
    ),
    (
        "ASSIGNABLE_USER",
        "Assignable User",
        "Users with this permission may be assigned to issues.",
    ),
    (
        "RESOLVE_ISSUES",
        "Resolve Issues",
        "Ability to resolve and reopen issues. This includes the ability to set a fix version.",
    ),
    (
        "CLOSE_ISSUES",
        "Close Issues",
        "Ability to close issues. Often useful where your developers resolve issues, and a QA department closes them.",
    ),
    (
        "MODIFY_REPORTER",
        "Modify Reporter",
        "Ability to modify the reporter when creating or editing an issue.",
    ),
    (
        "DELETE_ISSUES",
        "Delete Issues",
        "Ability to delete issues.",
    ),
    (
        "LINK_ISSUES",
        "Link Issues",
        "Ability to link issues together and create linked issues.",
    ),
    (
        "VIEW_VOTERS_AND_WATCHERS",
        "View Voters and Watchers",
        "Ability to view the voters and watchers of an issue.",
    ),
    (
        "MANAGE_WATCHERS",
        "Manage Watchers",
        "Ability to manage the watchers of an issue.",
    ),
    (
        "ADD_COMMENTS",
        "Add Comments",
        "Ability to comment on issues.",
    ),
    (
        "EDIT_ALL_COMMENTS",
        "Edit All Comments",
        "Ability to edit all comments made on issues.",
    ),
    (
        "EDIT_OWN_COMMENTS",
        "Edit Own Comments",
        "Ability to edit own comments made on issues.",
    ),
    (
        "DELETE_ALL_COMMENTS",
        "Delete All Comments",
        "Ability to delete all comments made on issues.",
    ),
    (
        "DELETE_OWN_COMMENTS",
        "Delete Own Comments",
        "Ability to delete own comments made on issues.",
    ),
    (
        "CREATE_ATTACHMENTS",
        "Create Attachments",
        "Users with this permission may create attachments.",
    ),
    (
        "DELETE_ALL_ATTACHMENTS",
        "Delete All Attachments",
        "Users with this permission may delete all attachments.",
    ),
    (
        "DELETE_OWN_ATTACHMENTS",
        "Delete Own Attachments",
        "Users with this permission may delete own attachments.",
    ),
    (
        "WORK_ON_ISSUES",
        "Work On Issues",
        "Ability to log work done against an issue.",
    ),
    (
        "EDIT_OWN_WORKLOGS",
        "Edit Own Worklogs",
        "Ability to edit own worklogs made on issues.",
    ),
    (
        "EDIT_ALL_WORKLOGS",
        "Edit All Worklogs",
        "Ability to edit all worklogs made on issues.",
    ),
    (
        "DELETE_OWN_WORKLOGS",
        "Delete Own Worklogs",
        "Ability to delete own worklogs made on issues.",
    ),
    (
        "DELETE_ALL_WORKLOGS",
        "Delete All Worklogs",
        "Ability to delete all worklogs made on issues.",
    ),
];

/// Global permissions, which only depend on who the user is.
pub const GLOBAL_PERMISSIONS: &[(&str, &str, &str)] = &[
    (
        "ADMINISTER",
        "Administer Jira",
        "Create and administer projects, issue types, fields, and schemes.",
    ),
    (
        "SYSTEM_ADMIN",
        "Jira System Administrators",
        "Ability to perform most administration functions.",
    ),
    (
        "USER_PICKER",
        "Browse users and groups",
        "Ability to select a user or group from a popup window.",
    ),
    (
        "CREATE_SHARED_OBJECTS",
        "Create Shared Objects",
        "Ability to share dashboards and filters with other users.",
    ),
    (
        "BULK_CHANGE",
        "Make bulk changes",
        "Ability to modify a collection of issues at once.",
    ),
];

pub fn is_permission(key: &str) -> bool {
    PROJECT_PERMISSIONS.iter().any(|(k, _, _)| *k == key)
}

/// Holder types a grant can name.
pub const HOLDER_TYPES: &[&str] = &[
    "anyone",
    "applicationRole",
    "group",
    "user",
    "projectRole",
    "projectLead",
    "reporter",
    "assignee",
];

/// What one user may do in one project.
pub struct Access {
    admin: bool,
    user_id: i64,
    account_id: String,
    /// Names and ids of the user's groups, in lower case.
    groups: HashSet<String>,
    roles: HashSet<i64>,
    lead_id: Option<i64>,
    grants: Vec<Grant>,
}

impl Access {
    pub async fn load(db: &SqlitePool, user: &User, project: &Project) -> ApiResult<Access> {
        let scheme = permissions::project_scheme(db, project.id).await?;
        let mut groups = HashSet::new();
        for group in groups::of_user(db, user.id).await? {
            groups.insert(group.name.to_lowercase());
            groups.insert(group.group_id.to_lowercase());
        }
        Ok(Access {
            admin: user.is_admin,
            user_id: user.id,
            account_id: user.account_id.clone(),
            groups,
            roles: permissions::roles_of_user(db, project.id, user.id)
                .await?
                .into_iter()
                .collect(),
            lead_id: project.lead_id,
            grants: permissions::grants(db, scheme).await?,
        })
    }

    fn holds(&self, grant: &Grant, issue: Option<&Issue>) -> bool {
        let param = grant.holder_parameter.as_deref().unwrap_or_default();
        match grant.holder_type.as_str() {
            "anyone" | "applicationRole" => true,
            "group" => self.groups.contains(&param.to_lowercase()),
            "user" => param == self.account_id,
            "projectRole" => param
                .parse()
                .is_ok_and(|role: i64| self.roles.contains(&role)),
            "projectLead" => self.lead_id == Some(self.user_id),
            "reporter" => issue.is_some_and(|i| i.reporter_id == Some(self.user_id)),
            "assignee" => issue.is_some_and(|i| i.assignee_id == Some(self.user_id)),
            _ => false,
        }
    }

    /// Whether the user has a permission in the project.
    pub fn can(&self, permission: &str) -> bool {
        self.can_on(permission, None)
    }

    /// Whether the user has a permission for one issue, where reporter and assignee grants count.
    pub fn can_on(&self, permission: &str, issue: Option<&Issue>) -> bool {
        self.admin
            || self
                .grants
                .iter()
                .any(|g| g.permission == permission && self.holds(g, issue))
    }

    /// Whether the user may act on something they own (`own`) or on anyone's (`all`).
    pub fn can_own_or_all(&self, own: &str, all: &str, is_own: bool, issue: &Issue) -> bool {
        self.can_on(all, Some(issue)) || (is_own && self.can_on(own, Some(issue)))
    }
}

/// The 403 message for a missing permission, in Jira's words.
fn denied(permission: &str) -> ApiError {
    let action = match permission {
        "CREATE_ISSUES" => "create issues in this project",
        "EDIT_ISSUES" => "edit issues in this project",
        "DELETE_ISSUES" => "delete issues in this project",
        "ASSIGN_ISSUES" => "assign issues in this project",
        "TRANSITION_ISSUES" => "transition issues in this project",
        "ADD_COMMENTS" => "comment on issues in this project",
        "CREATE_ATTACHMENTS" => "create attachments for this issue",
        "LINK_ISSUES" => "link issues in this project",
        "WORK_ON_ISSUES" => "log work on issues in this project",
        "MANAGE_WATCHERS" => "manage the watchers of this issue",
        "MANAGE_SPRINTS_PERMISSION" => "manage sprints in this project",
        "ADMINISTER_PROJECTS" => "administer this project",
        "EDIT_ALL_COMMENTS" => "edit this comment",
        "DELETE_ALL_COMMENTS" => "delete this comment",
        "EDIT_ALL_WORKLOGS" => "edit this worklog",
        "DELETE_ALL_WORKLOGS" => "delete this worklog",
        "DELETE_ALL_ATTACHMENTS" => "delete this attachment",
        _ => "do this",
    };
    ApiError::forbidden(format!("You do not have permission to {action}."))
}

/// Checks a project permission. A project the user cannot browse is reported as missing.
pub async fn require_project(
    state: &AppState,
    user: &User,
    project: &Project,
    permission: &str,
) -> ApiResult<Access> {
    let access = Access::load(&state.db, user, project).await?;
    if !access.can("BROWSE_PROJECTS") {
        return Err(ApiError::project_not_found(&project.key));
    }
    if !access.can(permission) {
        return Err(denied(permission));
    }
    Ok(access)
}

/// Checks a permission on an issue. An issue the user cannot browse is reported as missing.
pub async fn require_issue(
    state: &AppState,
    user: &User,
    issue: &Issue,
    permission: &str,
) -> ApiResult<Access> {
    let project = projects::by_id(&state.db, issue.project_id)
        .await?
        .ok_or_else(ApiError::issue_not_found)?;
    let access = Access::load(&state.db, user, &project).await?;
    if !access.can_on("BROWSE_PROJECTS", Some(issue)) {
        return Err(ApiError::issue_not_found());
    }
    if !access.can_on(permission, Some(issue)) {
        return Err(denied(permission));
    }
    Ok(access)
}

/// An issue the user may act on with `permission`.
pub async fn find_issue_as(
    state: &AppState,
    user: &User,
    id_or_key: &str,
    permission: &str,
) -> ApiResult<Issue> {
    let issue = issues::by_id_or_key(&state.db, id_or_key)
        .await?
        .ok_or_else(ApiError::issue_not_found)?;
    require_issue(state, user, &issue, permission).await?;
    Ok(issue)
}

/// An issue the user can browse, with what they may do to it.
pub async fn find_issue_access(
    state: &AppState,
    user: &User,
    id_or_key: &str,
) -> ApiResult<(Issue, Access)> {
    let issue = issues::by_id_or_key(&state.db, id_or_key)
        .await?
        .ok_or_else(ApiError::issue_not_found)?;
    let access = require_issue(state, user, &issue, "BROWSE_PROJECTS").await?;
    Ok((issue, access))
}

/// Fails with 403 unless `allowed`, using `permission` for the message.
pub fn check(allowed: bool, permission: &str) -> ApiResult<()> {
    if allowed {
        Ok(())
    } else {
        Err(denied(permission))
    }
}

/// Ids of the projects a user can browse, or `None` when they can browse every project.
pub async fn browsable_projects(db: &SqlitePool, user: &User) -> ApiResult<Option<Vec<i64>>> {
    if user.is_admin {
        return Ok(None);
    }
    let all = projects::list(db).await?;
    let mut ids = Vec::with_capacity(all.len());
    let mut every = true;
    for project in &all {
        if Access::load(db, user, project)
            .await?
            .can("BROWSE_PROJECTS")
        {
            ids.push(project.id);
        } else {
            every = false;
        }
    }
    Ok(if every { None } else { Some(ids) })
}
