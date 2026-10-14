-- Email waiting to be sent. A background task delivers it when SMTP is configured.
CREATE TABLE outbox (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    to_email    TEXT    NOT NULL,
    subject     TEXT    NOT NULL,
    body        TEXT    NOT NULL,
    created_at  INTEGER NOT NULL,
    sent_at     INTEGER,
    attempts    INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT
);
CREATE INDEX outbox_pending ON outbox (sent_at, attempts);
