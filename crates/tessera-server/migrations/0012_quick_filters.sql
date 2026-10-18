-- Quick filters: named JQL that narrows a board when switched on.
CREATE TABLE quick_filters (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    board_id     INTEGER NOT NULL REFERENCES boards (id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    jql          TEXT    NOT NULL,
    description  TEXT    NOT NULL DEFAULT '',
    position     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX quick_filters_board ON quick_filters (board_id, position);

-- Every board starts with Jira's two quick filters.
INSERT INTO quick_filters (board_id, name, jql, position)
SELECT id, 'Only My Issues', 'assignee = currentUser()', 0 FROM boards;
INSERT INTO quick_filters (board_id, name, jql, position)
SELECT id, 'Recently Updated', 'updatedDate >= -1d', 1 FROM boards;
