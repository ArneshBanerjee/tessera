-- Keys issues had before they moved to another project. Jira keeps them working.
CREATE TABLE moved_issue_keys (
    old_key  TEXT    PRIMARY KEY COLLATE NOCASE,
    issue_id INTEGER NOT NULL REFERENCES issues (id) ON DELETE CASCADE
);

-- Bulk operations and what came of them, for `/bulk/queue/{taskId}`.
CREATE TABLE bulk_tasks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- JSON: the processed issue ids, the failures by issue id, and the counts.
    result       TEXT    NOT NULL,
    created_at   INTEGER NOT NULL
);
INSERT INTO sqlite_sequence (name, seq) VALUES ('bulk_tasks', 10000);
