-- Confluence space permissions: who may do which operation in a space.
--
-- Operations are (operation, target) pairs as in Confluence's API, for example
-- ('read', 'space'), ('create', 'page'), ('delete', 'attachment'), ('administer', 'space').
-- ('delete', 'space') is "Delete own": removing your own pages, comments, and attachments.
CREATE TABLE space_permissions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    space_id      INTEGER NOT NULL REFERENCES spaces (id) ON DELETE CASCADE,
    -- 'user' (subject: account id), 'group' (subject: group name), or 'anonymous'.
    subject_type  TEXT    NOT NULL,
    subject       TEXT    NOT NULL DEFAULT '',
    operation     TEXT    NOT NULL,
    target        TEXT    NOT NULL,
    UNIQUE (space_id, subject_type, subject, operation, target)
);
CREATE INDEX space_permissions_space ON space_permissions (space_id);

INSERT INTO sqlite_sequence (name, seq) VALUES ('space_permissions', 0);

-- Confluence's default group. Every user belongs to it; new users join it when created.
INSERT INTO groups (group_id, name) VALUES (lower(hex(randomblob(16))), 'confluence-users');
INSERT INTO group_members (group_id, user_id)
SELECT g.id, u.id FROM groups g, users u WHERE g.name = 'confluence-users';

-- Existing spaces get Confluence's defaults: the group works in the space, the creator
-- administers it.
INSERT INTO space_permissions (space_id, subject_type, subject, operation, target)
SELECT s.id, 'group', 'confluence-users', op.operation, op.target
FROM spaces s, (
    SELECT 'read' AS operation, 'space' AS target UNION ALL SELECT 'create', 'page' UNION ALL
    SELECT 'create', 'blogpost' UNION ALL SELECT 'create', 'comment' UNION ALL
    SELECT 'create', 'attachment' UNION ALL SELECT 'delete', 'space' UNION ALL
    SELECT 'export', 'space'
) op;

INSERT INTO space_permissions (space_id, subject_type, subject, operation, target)
SELECT s.id, 'user', u.account_id, op.operation, op.target
FROM spaces s JOIN users u ON u.id = s.creator_id, (
    SELECT 'read' AS operation, 'space' AS target UNION ALL SELECT 'create', 'page' UNION ALL
    SELECT 'create', 'blogpost' UNION ALL SELECT 'create', 'comment' UNION ALL
    SELECT 'create', 'attachment' UNION ALL SELECT 'delete', 'space' UNION ALL
    SELECT 'delete', 'page' UNION ALL SELECT 'delete', 'blogpost' UNION ALL
    SELECT 'delete', 'comment' UNION ALL SELECT 'delete', 'attachment' UNION ALL
    SELECT 'export', 'space' UNION ALL SELECT 'restrict_content', 'space' UNION ALL
    SELECT 'archive', 'page' UNION ALL SELECT 'administer', 'space'
) op;
