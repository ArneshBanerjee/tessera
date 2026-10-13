-- Saved filters, and Jira Software: boards, columns, sprints, epics, ranking, and properties.

CREATE TABLE filters (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    jql         TEXT    NOT NULL,
    owner_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    UNIQUE (owner_id, name)
);

CREATE TABLE filter_favourites (
    filter_id INTEGER NOT NULL REFERENCES filters (id) ON DELETE CASCADE,
    user_id   INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (filter_id, user_id)
);

-- Who else can see a filter: 'global' (everyone), 'authenticated' (signed-in users), or
-- 'project' (members of project_id; today, every signed-in user).
CREATE TABLE filter_shares (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    filter_id  INTEGER NOT NULL REFERENCES filters (id) ON DELETE CASCADE,
    share_type TEXT    NOT NULL,
    project_id INTEGER REFERENCES projects (id) ON DELETE CASCADE
);

CREATE TABLE boards (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL,
    -- 'scrum' or 'kanban'.
    board_type TEXT    NOT NULL,
    filter_id  INTEGER NOT NULL REFERENCES filters (id),
    -- The project the board lives in (its "location").
    project_id INTEGER REFERENCES projects (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
);

CREATE TABLE board_columns (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    board_id   INTEGER NOT NULL REFERENCES boards (id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    name       TEXT    NOT NULL,
    min_issues INTEGER,
    max_issues INTEGER
);

CREATE TABLE board_column_statuses (
    column_id INTEGER NOT NULL REFERENCES board_columns (id) ON DELETE CASCADE,
    status_id INTEGER NOT NULL REFERENCES statuses (id) ON DELETE CASCADE,
    PRIMARY KEY (column_id, status_id)
);

CREATE TABLE sprints (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT    NOT NULL,
    -- 'future', 'active', or 'closed'.
    state           TEXT    NOT NULL DEFAULT 'future',
    goal            TEXT    NOT NULL DEFAULT '',
    start_date      INTEGER,
    end_date        INTEGER,
    complete_date   INTEGER,
    origin_board_id INTEGER REFERENCES boards (id) ON DELETE SET NULL,
    -- Order of sprints on their board, used by swap.
    sequence        INTEGER NOT NULL,
    created_at      INTEGER NOT NULL
);

-- An issue keeps its closed sprints as history and is in at most one open sprint.
CREATE TABLE issue_sprints (
    issue_id  INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE,
    sprint_id INTEGER NOT NULL REFERENCES sprints (id) ON DELETE CASCADE,
    PRIMARY KEY (issue_id, sprint_id)
);
CREATE INDEX issue_sprints_sprint ON issue_sprints (sprint_id);

-- Agile details of epic issues. Name defaults to the summary; done to the status category.
CREATE TABLE epics (
    issue_id  INTEGER PRIMARY KEY REFERENCES issues (id) ON DELETE CASCADE,
    name      TEXT,
    color_key TEXT NOT NULL DEFAULT 'color_1',
    done      INTEGER
);

-- Key-value properties of boards and sprints (JSON values).
CREATE TABLE entity_properties (
    entity_type TEXT    NOT NULL,
    entity_id   INTEGER NOT NULL,
    key         TEXT    NOT NULL,
    value       TEXT    NOT NULL,
    PRIMARY KEY (entity_type, entity_id, key)
);

-- Global issue order for boards and backlogs. Ranks compare as strings.
ALTER TABLE issues ADD COLUMN lexo_rank TEXT;
CREATE INDEX issues_rank ON issues (lexo_rank);

INSERT INTO sqlite_sequence (name, seq) VALUES
    ('filters', 9999), ('filter_shares', 9999), ('boards', 0), ('board_columns', 0), ('sprints', 0);

-- Jira Software's fields. Clients find them by name in /field.
INSERT INTO custom_fields (name, description, field_type) VALUES
    ('Sprint', 'Jira Software sprint field', 'sprint'),
    ('Rank', 'Global rank field for Jira Software use only.', 'rank'),
    ('Story point estimate', 'Measurement of complexity and/or size of a requirement.', 'number');
