-- Migration 0005: Markierungen und Notizen im Lesemodus.
-- Eine Markierung gehört zu genau einem Artikel und Benutzer. Sie wird über Zeichenpositionen
-- im dargestellten Text verankert (start_offset/end_offset, in UTF-16-Einheiten wie in
-- JavaScript) und zusätzlich über den markierten Wortlaut mit etwas Umgebung (quote, prefix,
-- suffix). Damit lässt sich die Stelle auch wiederfinden, falls sich die Darstellung ändert.
-- Die Farbe ist ein Hex-Wert (#rrggbb), die Notiz ist optional.

CREATE TABLE highlights (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    article_id   INTEGER NOT NULL REFERENCES articles (id) ON DELETE CASCADE,
    start_offset INTEGER NOT NULL CHECK (start_offset >= 0),
    end_offset   INTEGER NOT NULL CHECK (end_offset > start_offset),
    quote        TEXT    NOT NULL,
    prefix       TEXT    NOT NULL DEFAULT '',
    suffix       TEXT    NOT NULL DEFAULT '',
    color        TEXT    NOT NULL CHECK (length(color) = 7 AND substr(color, 1, 1) = '#'),
    note         TEXT,
    created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE INDEX idx_highlights_article ON highlights (article_id, start_offset);
CREATE INDEX idx_highlights_user ON highlights (user_id);
