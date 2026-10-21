-- In-app notifications: what each person heard about, and whether they read it.

CREATE TABLE notifications (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    actor_id   INTEGER REFERENCES users (id) ON DELETE SET NULL,
    -- 'issue', 'page', or 'mention'.
    kind       TEXT    NOT NULL,
    title      TEXT    NOT NULL,
    body       TEXT    NOT NULL DEFAULT '',
    -- The web app path the notification opens.
    link       TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    read_at    INTEGER
);

CREATE INDEX notifications_user ON notifications (user_id, id DESC);
