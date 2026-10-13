-- Jira depth: components, versions, watchers, votes, issue links, worklogs, and attachments.

CREATE TABLE components (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id    INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name          TEXT    NOT NULL COLLATE NOCASE,
    description   TEXT    NOT NULL DEFAULT '',
    lead_id       INTEGER REFERENCES users (id) ON DELETE SET NULL,
    -- PROJECT_DEFAULT, COMPONENT_LEAD, PROJECT_LEAD, or UNASSIGNED.
    assignee_type TEXT    NOT NULL DEFAULT 'PROJECT_DEFAULT',
    UNIQUE (project_id, name)
);

CREATE TABLE versions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id   INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name         TEXT    NOT NULL COLLATE NOCASE,
    description  TEXT    NOT NULL DEFAULT '',
    archived     INTEGER NOT NULL DEFAULT 0,
    released     INTEGER NOT NULL DEFAULT 0,
    -- yyyy-MM-dd.
    start_date   TEXT,
    release_date TEXT,
    -- Display order within the project.
    sequence     INTEGER NOT NULL,
    UNIQUE (project_id, name)
);

CREATE TABLE issue_components (
    issue_id     INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    component_id INTEGER NOT NULL REFERENCES components (id) ON DELETE CASCADE,
    PRIMARY KEY (issue_id, component_id)
);

-- kind is 'fix' (fixVersions) or 'affects' (versions).
CREATE TABLE issue_versions (
    issue_id   INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    version_id INTEGER NOT NULL REFERENCES versions (id) ON DELETE CASCADE,
    kind       TEXT    NOT NULL,
    PRIMARY KEY (issue_id, version_id, kind)
);

CREATE TABLE watchers (
    issue_id INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    user_id  INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (issue_id, user_id)
);

CREATE TABLE votes (
    issue_id INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    user_id  INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (issue_id, user_id)
);

CREATE TABLE issue_link_types (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    name    TEXT NOT NULL UNIQUE COLLATE NOCASE,
    -- How the inward issue relates, for example 'is blocked by'.
    inward  TEXT NOT NULL,
    -- How the outward issue relates, for example 'blocks'.
    outward TEXT NOT NULL
);

-- "outward issue <outward> inward issue", for example "PROJ-1 blocks PROJ-2".
CREATE TABLE issue_links (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    link_type_id     INTEGER NOT NULL REFERENCES issue_link_types (id) ON DELETE CASCADE,
    outward_issue_id INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    inward_issue_id  INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    created_at       INTEGER NOT NULL,
    UNIQUE (link_type_id, outward_issue_id, inward_issue_id)
);
CREATE INDEX issue_links_outward ON issue_links (outward_issue_id);
CREATE INDEX issue_links_inward ON issue_links (inward_issue_id);

CREATE TABLE worklogs (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id           INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    author_id          INTEGER NOT NULL REFERENCES users (id),
    update_author_id   INTEGER NOT NULL REFERENCES users (id),
    -- ADF JSON, and plain text for search.
    comment            TEXT,
    comment_text       TEXT    NOT NULL DEFAULT '',
    started            INTEGER NOT NULL,
    time_spent_seconds INTEGER NOT NULL,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);
CREATE INDEX worklogs_issue ON worklogs (issue_id);

-- Time tracking estimates, in seconds.
ALTER TABLE issues ADD COLUMN original_estimate INTEGER;
ALTER TABLE issues ADD COLUMN remaining_estimate INTEGER;

-- File contents live in the data directory, named by id.
CREATE TABLE attachments (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id   INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    author_id  INTEGER NOT NULL REFERENCES users (id),
    filename   TEXT    NOT NULL,
    mime_type  TEXT    NOT NULL,
    size       INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX attachments_issue ON attachments (issue_id);

INSERT INTO sqlite_sequence (name, seq) VALUES
    ('components', 9999), ('versions', 9999), ('issue_link_types', 9999),
    ('issue_links', 9999), ('worklogs', 9999), ('attachments', 9999);

-- The link types of a new Jira site.
INSERT INTO issue_link_types (name, inward, outward) VALUES
    ('Blocks', 'is blocked by', 'blocks'),
    ('Cloners', 'is cloned by', 'clones'),
    ('Duplicate', 'is duplicated by', 'duplicates'),
    ('Relates', 'relates to', 'relates to');
