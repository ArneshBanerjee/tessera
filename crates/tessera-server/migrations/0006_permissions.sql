-- Groups, project roles, and permission schemes, as in Jira.

CREATE TABLE groups (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Jira's opaque group id.
    group_id  TEXT    NOT NULL UNIQUE,
    name      TEXT    NOT NULL UNIQUE COLLATE NOCASE
);

CREATE TABLE group_members (
    group_id  INTEGER NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    user_id   INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, user_id)
);

CREATE TABLE project_roles (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    description  TEXT    NOT NULL DEFAULT ''
);

-- Who holds a role in a project: users directly, or every member of a group.
CREATE TABLE project_role_users (
    project_id  INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    role_id     INTEGER NOT NULL REFERENCES project_roles (id) ON DELETE CASCADE,
    user_id     INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (project_id, role_id, user_id)
);

CREATE TABLE project_role_groups (
    project_id  INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    role_id     INTEGER NOT NULL REFERENCES project_roles (id) ON DELETE CASCADE,
    group_id    INTEGER NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    PRIMARY KEY (project_id, role_id, group_id)
);

CREATE TABLE permission_schemes (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    description  TEXT    NOT NULL DEFAULT ''
);

-- One grant gives one permission to one holder. Holder types: anyone, applicationRole,
-- group (parameter: group name), user (account id), projectRole (role id), projectLead,
-- reporter, and assignee.
CREATE TABLE permission_grants (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    scheme_id         INTEGER NOT NULL REFERENCES permission_schemes (id) ON DELETE CASCADE,
    permission        TEXT    NOT NULL,
    holder_type       TEXT    NOT NULL,
    holder_parameter  TEXT
);
CREATE INDEX permission_grants_scheme ON permission_grants (scheme_id, permission);

-- SQLite does not allow a foreign key on an added column with a default, so the code checks it.
ALTER TABLE projects ADD COLUMN permission_scheme_id INTEGER NOT NULL DEFAULT 0;

INSERT INTO sqlite_sequence (name, seq) VALUES
    ('groups', 0), ('project_roles', 10001), ('permission_schemes', 9999), ('permission_grants', 10099);

INSERT INTO project_roles (name, description) VALUES
    ('Administrators', 'A project role that represents administrators in a project'),
    ('Members', 'A project role that represents members in a project'),
    ('Viewers', 'A project role that represents viewers in a project');

-- Jira's default scheme has id 0: every signed-in user works in projects; the project's
-- Administrators role administers them and does the destructive things.
INSERT INTO permission_schemes (id, name, description) VALUES
    (0, 'Default Permission Scheme', 'This is the default Permission Scheme. Any new projects that are created will be assigned this scheme.');

INSERT INTO permission_grants (scheme_id, permission, holder_type, holder_parameter)
SELECT 0, permission, 'applicationRole', NULL FROM (
    SELECT 'BROWSE_PROJECTS' AS permission UNION ALL SELECT 'CREATE_ISSUES' UNION ALL
    SELECT 'EDIT_ISSUES' UNION ALL SELECT 'TRANSITION_ISSUES' UNION ALL SELECT 'SCHEDULE_ISSUES' UNION ALL
    SELECT 'MOVE_ISSUES' UNION ALL SELECT 'ASSIGN_ISSUES' UNION ALL SELECT 'ASSIGNABLE_USER' UNION ALL
    SELECT 'RESOLVE_ISSUES' UNION ALL SELECT 'CLOSE_ISSUES' UNION ALL SELECT 'LINK_ISSUES' UNION ALL
    SELECT 'VIEW_VOTERS_AND_WATCHERS' UNION ALL SELECT 'ADD_COMMENTS' UNION ALL
    SELECT 'EDIT_OWN_COMMENTS' UNION ALL SELECT 'DELETE_OWN_COMMENTS' UNION ALL
    SELECT 'CREATE_ATTACHMENTS' UNION ALL SELECT 'DELETE_OWN_ATTACHMENTS' UNION ALL
    SELECT 'WORK_ON_ISSUES' UNION ALL SELECT 'EDIT_OWN_WORKLOGS' UNION ALL
    SELECT 'DELETE_OWN_WORKLOGS' UNION ALL SELECT 'MANAGE_SPRINTS_PERMISSION' UNION ALL
    SELECT 'VIEW_READONLY_WORKFLOW'
);

INSERT INTO permission_grants (scheme_id, permission, holder_type, holder_parameter)
SELECT 0, permission, 'projectRole', '10002' FROM (
    SELECT 'ADMINISTER_PROJECTS' AS permission UNION ALL SELECT 'DELETE_ISSUES' UNION ALL
    SELECT 'MODIFY_REPORTER' UNION ALL SELECT 'MANAGE_WATCHERS' UNION ALL
    SELECT 'EDIT_ALL_COMMENTS' UNION ALL SELECT 'DELETE_ALL_COMMENTS' UNION ALL
    SELECT 'DELETE_ALL_ATTACHMENTS' UNION ALL SELECT 'EDIT_ALL_WORKLOGS' UNION ALL
    SELECT 'DELETE_ALL_WORKLOGS'
);

-- Project leads administer the projects they already lead.
INSERT INTO project_role_users (project_id, role_id, user_id)
SELECT id, 10002, lead_id FROM projects WHERE lead_id IS NOT NULL;
