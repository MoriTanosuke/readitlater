-- Migration 0004: Tokens zum Teilen von Artikeln aus anderen Apps
-- (Android: HTTP Shortcuts, iPad/iPhone: Kurzbefehle).
-- Gespeichert wird nur der SHA-256-Hash. Ein Token darf ausschließlich
-- POST /api/share aufrufen, nicht lesen oder löschen.

CREATE TABLE share_tokens (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    token_hash   TEXT    NOT NULL UNIQUE,
    created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    last_used_at TEXT
);
CREATE INDEX idx_share_tokens_user ON share_tokens (user_id);
