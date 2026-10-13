-- Jira core: users, auth, issue metadata, workflows, projects, issues, comments, and history.
-- Timestamps are milliseconds since the Unix epoch (UTC). Rich text is stored as ADF JSON.

CREATE TABLE users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id    TEXT    NOT NULL UNIQUE,
    email         TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    display_name  TEXT    NOT NULL,
    password_hash TEXT,
    time_zone     TEXT    NOT NULL DEFAULT 'UTC',
    active        INTEGER NOT NULL DEFAULT 1,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL
);

-- API tokens for basic auth (email + token), like Jira Cloud. Only the SHA-256 hash is stored.
CREATE TABLE api_tokens (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    label        TEXT    NOT NULL,
    token_hash   TEXT    NOT NULL UNIQUE,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER
);

-- Browser sessions for the web app. Only the SHA-256 hash of the cookie value is stored.
CREATE TABLE sessions (
    token_hash TEXT    PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
);

-- The four fixed status categories, with the ids and keys Jira uses.
CREATE TABLE status_categories (
    id         INTEGER PRIMARY KEY,
    key        TEXT NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    color_name TEXT NOT NULL
);
INSERT INTO status_categories (id, key, name, color_name) VALUES
    (1, 'undefined', 'No Category', 'medium-gray'),
    (2, 'new', 'To Do', 'blue-gray'),
    (4, 'indeterminate', 'In Progress', 'yellow'),
    (3, 'done', 'Done', 'green');

CREATE TABLE statuses (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    description TEXT    NOT NULL DEFAULT '',
    category_id INTEGER NOT NULL REFERENCES status_categories (id)
);

CREATE TABLE issue_types (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    description     TEXT    NOT NULL DEFAULT '',
    -- 1 = epic, 0 = standard, -1 = subtask (Jira's hierarchy levels).
    hierarchy_level INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE priorities (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    description TEXT    NOT NULL DEFAULT '',
    status_color TEXT   NOT NULL,
    -- Lower sequence = more urgent. Used to sort by priority.
    sequence    INTEGER NOT NULL
);

CREATE TABLE resolutions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    description TEXT NOT NULL DEFAULT ''
);

CREATE TABLE workflows (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT ''
);

-- The statuses a workflow uses. The status with is_initial = 1 is where new issues start.
CREATE TABLE workflow_statuses (
    workflow_id INTEGER NOT NULL REFERENCES workflows (id) ON DELETE CASCADE,
    status_id   INTEGER NOT NULL REFERENCES statuses (id),
    is_initial  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (workflow_id, status_id)
);

-- A transition with from_status_id NULL is global: it is available from every status.
CREATE TABLE workflow_transitions (
    id             INTEGER PRIMARY KEY,
    workflow_id    INTEGER NOT NULL REFERENCES workflows (id) ON DELETE CASCADE,
    name           TEXT    NOT NULL,
    from_status_id INTEGER REFERENCES statuses (id),
    to_status_id   INTEGER NOT NULL REFERENCES statuses (id)
);

CREATE TABLE project_categories (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    description TEXT NOT NULL DEFAULT ''
);

CREATE TABLE projects (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    key              TEXT    NOT NULL UNIQUE,
    name             TEXT    NOT NULL,
    description      TEXT    NOT NULL DEFAULT '',
    lead_id          INTEGER REFERENCES users (id),
    category_id      INTEGER REFERENCES project_categories (id) ON DELETE SET NULL,
    url              TEXT    NOT NULL DEFAULT '',
    -- Who new issues go to when nobody is picked: PROJECT_LEAD or UNASSIGNED.
    assignee_type    TEXT    NOT NULL DEFAULT 'UNASSIGNED',
    project_type_key TEXT    NOT NULL DEFAULT 'software',
    workflow_id      INTEGER NOT NULL REFERENCES workflows (id),
    -- The number used by the most recent issue key, for example 41 in PROJ-41.
    issue_counter    INTEGER NOT NULL DEFAULT 0,
    created_at       INTEGER NOT NULL
);

CREATE TABLE issues (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id      INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    number          INTEGER NOT NULL,
    key             TEXT    NOT NULL UNIQUE,
    issue_type_id   INTEGER NOT NULL REFERENCES issue_types (id),
    summary         TEXT    NOT NULL,
    description     TEXT,
    environment     TEXT,
    -- Plain-text copies of the rich-text fields, for JQL text search.
    description_text TEXT NOT NULL DEFAULT '',
    environment_text TEXT NOT NULL DEFAULT '',
    status_id       INTEGER NOT NULL REFERENCES statuses (id),
    priority_id     INTEGER REFERENCES priorities (id),
    resolution_id   INTEGER REFERENCES resolutions (id),
    resolution_date INTEGER,
    assignee_id     INTEGER REFERENCES users (id),
    reporter_id     INTEGER REFERENCES users (id),
    creator_id      INTEGER NOT NULL REFERENCES users (id),
    -- Children of a deleted epic lose their parent. Subtasks are deleted explicitly first.
    parent_id       INTEGER REFERENCES issues (id) ON DELETE SET NULL,
    due_date        TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    UNIQUE (project_id, number)
);
CREATE INDEX issues_project ON issues (project_id);
CREATE INDEX issues_assignee ON issues (assignee_id);
CREATE INDEX issues_parent ON issues (parent_id);

CREATE TABLE issue_labels (
    issue_id INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    label    TEXT    NOT NULL,
    PRIMARY KEY (issue_id, label)
);

CREATE TABLE custom_fields (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    -- One of: string, text, number, date, datetime, url, user, option, options, labels.
    field_type  TEXT NOT NULL
);

CREATE TABLE custom_field_options (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    field_id INTEGER NOT NULL REFERENCES custom_fields (id) ON DELETE CASCADE,
    value    TEXT    NOT NULL,
    position INTEGER NOT NULL,
    disabled INTEGER NOT NULL DEFAULT 0
);

-- Custom field values as JSON, in the shape the REST API returns them.
CREATE TABLE issue_field_values (
    issue_id INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    field_id INTEGER NOT NULL REFERENCES custom_fields (id) ON DELETE CASCADE,
    value    TEXT    NOT NULL,
    PRIMARY KEY (issue_id, field_id)
);

CREATE TABLE comments (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id         INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    author_id        INTEGER NOT NULL REFERENCES users (id),
    update_author_id INTEGER NOT NULL REFERENCES users (id),
    body             TEXT    NOT NULL,
    body_text        TEXT    NOT NULL,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
);
CREATE INDEX comments_issue ON comments (issue_id);

-- Issue history. JQL's WAS and CHANGED operators search these tables.
CREATE TABLE changelogs (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id   INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    author_id  INTEGER NOT NULL REFERENCES users (id),
    created_at INTEGER NOT NULL
);
CREATE INDEX changelogs_issue ON changelogs (issue_id);

CREATE TABLE changelog_items (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    changelog_id INTEGER NOT NULL REFERENCES changelogs (id) ON DELETE CASCADE,
    -- The field name as Jira shows it in history, for example 'status' or 'assignee'.
    field        TEXT    NOT NULL,
    field_id     TEXT    NOT NULL,
    field_type   TEXT    NOT NULL DEFAULT 'jira',
    from_value   TEXT,
    from_string  TEXT,
    to_value     TEXT,
    to_string    TEXT
);
CREATE INDEX changelog_items_changelog ON changelog_items (changelog_id);

-- Jira ids start at 10000. Seed the AUTOINCREMENT counters so ours do too.
INSERT INTO sqlite_sequence (name, seq) VALUES
    ('users', 9999), ('statuses', 9999), ('issue_types', 9999), ('resolutions', 9999),
    ('workflows', 0), ('projects', 9999), ('project_categories', 9999), ('issues', 9999), ('custom_fields', 9999),
    ('custom_field_options', 9999), ('comments', 9999), ('changelogs', 9999),
    ('changelog_items', 9999), ('api_tokens', 0), ('priorities', 0);

-- Default metadata, matching a new Jira site.
INSERT INTO statuses (name, category_id) VALUES ('To Do', 2), ('In Progress', 4), ('Done', 3);

INSERT INTO issue_types (name, description, hierarchy_level) VALUES
    ('Epic', 'A big user story that needs to be broken down.', 1),
    ('Story', 'Functionality or a feature expressed as a user goal.', 0),
    ('Task', 'A small, distinct piece of work.', 0),
    ('Bug', 'A problem or error.', 0),
    ('Subtask', 'A small piece of work that is part of a larger task.', -1);

INSERT INTO priorities (name, description, status_color, sequence) VALUES
    ('Highest', 'This problem will block progress.', '#d04437', 1),
    ('High', 'Serious problem that could block progress.', '#f15C75', 2),
    ('Medium', 'Has the potential to affect progress.', '#f79232', 3),
    ('Low', 'Minor problem or easily worked around.', '#707070', 4),
    ('Lowest', 'Trivial problem with little or no impact on progress.', '#999999', 5);

INSERT INTO resolutions (name, description) VALUES
    ('Done', 'Work has been completed on this issue.'),
    ('Won''t Do', 'This issue won''t be actioned.'),
    ('Duplicate', 'The problem is a duplicate of an existing issue.'),
    ('Cannot Reproduce', 'All attempts at reproducing this issue failed.');

-- The simplified workflow: every status can move to every other status.
INSERT INTO workflows (name, description) VALUES
    ('Simplified Workflow', 'Issues can move freely between To Do, In Progress, and Done.');
INSERT INTO workflow_statuses (workflow_id, status_id, is_initial) VALUES (1, 10000, 1), (1, 10001, 0), (1, 10002, 0);
INSERT INTO workflow_transitions (id, workflow_id, name, from_status_id, to_status_id) VALUES
    (11, 1, 'To Do', NULL, 10000),
    (21, 1, 'In Progress', NULL, 10001),
    (31, 1, 'Done', NULL, 10002);
