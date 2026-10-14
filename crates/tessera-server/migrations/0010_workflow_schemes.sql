-- Workflow schemes, as in Jira: a project uses one scheme, and the scheme picks the workflow
-- for each issue type, with a default for the rest.
--
-- projects.workflow_id from the first migration is no longer read; the scheme decides.

CREATE TABLE workflow_schemes (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    name                 TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    description          TEXT    NOT NULL DEFAULT '',
    default_workflow_id  INTEGER NOT NULL REFERENCES workflows (id)
);

CREATE TABLE workflow_scheme_mappings (
    scheme_id      INTEGER NOT NULL REFERENCES workflow_schemes (id) ON DELETE CASCADE,
    issue_type_id  INTEGER NOT NULL REFERENCES issue_types (id) ON DELETE CASCADE,
    workflow_id    INTEGER NOT NULL REFERENCES workflows (id),
    PRIMARY KEY (scheme_id, issue_type_id)
);

INSERT INTO sqlite_sequence (name, seq) VALUES ('workflow_schemes', 9999);

INSERT INTO workflow_schemes (name, description, default_workflow_id) VALUES
    ('Default Workflow Scheme', 'Every issue type uses the Simplified Workflow.', 1);

-- SQLite does not allow a foreign key on an added column with a default, so the code checks it.
ALTER TABLE projects ADD COLUMN workflow_scheme_id INTEGER NOT NULL DEFAULT 10000;
