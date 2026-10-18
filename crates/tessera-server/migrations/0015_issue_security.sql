-- Issue security: a project's security scheme has levels, and an issue at a level is seen
-- only by the level's members.

CREATE TABLE security_schemes (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    description      TEXT    NOT NULL DEFAULT '',
    -- The level new issues get when nobody picks one.
    default_level_id INTEGER
);

CREATE TABLE security_levels (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    scheme_id   INTEGER NOT NULL REFERENCES security_schemes (id) ON DELETE CASCADE,
    name        TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT ''
);

-- Members use the holder types of permission grants: anyone, applicationRole, group,
-- user, projectRole, projectLead, reporter, and assignee.
CREATE TABLE security_level_members (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    level_id         INTEGER NOT NULL REFERENCES security_levels (id) ON DELETE CASCADE,
    holder_type      TEXT    NOT NULL,
    holder_parameter TEXT
);

ALTER TABLE projects ADD COLUMN security_scheme_id INTEGER REFERENCES security_schemes (id) ON DELETE SET NULL;
ALTER TABLE issues ADD COLUMN security_level_id INTEGER REFERENCES security_levels (id) ON DELETE SET NULL;
CREATE INDEX issues_security_level ON issues (security_level_id);

-- An empty scheme, so clients that pick a scheme when creating projects find one.
INSERT INTO security_schemes (id, name, description) VALUES
    (10000, 'Default issue security scheme', 'A scheme without levels: everyone who can browse the project sees its issues.');
INSERT INTO sqlite_sequence (name, seq) VALUES ('security_levels', 9999), ('security_level_members', 9999);

-- Setting a security level is for project administrators, as in Jira's default scheme.
INSERT INTO permission_grants (scheme_id, permission, holder_type, holder_parameter)
    VALUES (0, 'SET_ISSUE_SECURITY', 'projectRole', '10002');
