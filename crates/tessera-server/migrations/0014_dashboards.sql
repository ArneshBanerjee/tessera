-- Jira dashboards: pages of gadgets that people build, share, and mark as favourites.

CREATE TABLE dashboards (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    -- NULL for the system dashboard, which administrators edit.
    owner_id    INTEGER REFERENCES users (id) ON DELETE CASCADE,
    -- Column widths, one letter per column, as Jira names layouts: 'A', 'AA', 'AB', 'BA', 'AAA'.
    layout      TEXT    NOT NULL DEFAULT 'AA',
    refresh_ms  INTEGER,
    created_at  INTEGER NOT NULL
);

-- Who else can see a dashboard, as for filters.
CREATE TABLE dashboard_shares (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    dashboard_id INTEGER NOT NULL REFERENCES dashboards (id) ON DELETE CASCADE,
    share_type   TEXT    NOT NULL,
    project_id   INTEGER REFERENCES projects (id) ON DELETE CASCADE
);

CREATE TABLE dashboard_favourites (
    dashboard_id INTEGER NOT NULL REFERENCES dashboards (id) ON DELETE CASCADE,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (dashboard_id, user_id)
);

CREATE TABLE dashboard_gadgets (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    dashboard_id INTEGER NOT NULL REFERENCES dashboards (id) ON DELETE CASCADE,
    module_key   TEXT,
    uri          TEXT,
    title        TEXT    NOT NULL DEFAULT '',
    color        TEXT    NOT NULL DEFAULT 'blue',
    col          INTEGER NOT NULL,
    row          INTEGER NOT NULL
);

CREATE INDEX dashboard_gadgets_dashboard ON dashboard_gadgets (dashboard_id, col, row);

-- Gadget settings. Values are JSON text.
CREATE TABLE dashboard_item_properties (
    gadget_id INTEGER NOT NULL REFERENCES dashboard_gadgets (id) ON DELETE CASCADE,
    key       TEXT    NOT NULL,
    value     TEXT    NOT NULL,
    PRIMARY KEY (gadget_id, key)
);

-- The system dashboard, which everyone sees until they make their own.
INSERT INTO dashboards (id, name, owner_id, layout, created_at)
    VALUES (10000, 'Default dashboard', NULL, 'AA', 0);
INSERT INTO dashboard_shares (dashboard_id, share_type) VALUES (10000, 'global');
INSERT INTO dashboard_gadgets (id, dashboard_id, module_key, title, color, col, row) VALUES
    (10000, 10000, 'com.atlassian.jira.gadgets:introduction-dashboard-item', 'Introduction', 'blue', 0, 0),
    (10001, 10000, 'com.atlassian.jira.gadgets:assigned-to-me-dashboard-item', 'Assigned to me', 'blue', 1, 0);

