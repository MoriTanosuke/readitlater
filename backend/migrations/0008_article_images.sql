-- Migration 0008: Bilder der Artikel lokal ablegen.
-- Eine Zeile je Bild eines Artikels (Original-URL aus dem gespeicherten HTML) und zugleich die
-- Warteschlange des Hintergrund-Workers (images.rs): pending -> working -> done oder failed.
-- Das gespeicherte Artikel-HTML bleibt unverändert; das Frontend tauscht die Adresse beim
-- Anzeigen gegen die lokale Fassung aus. Die Bilddaten sind schon verkleinert (imageproc.rs)
-- und stehen als BLOB in der Datenbank, damit Löschen (ON DELETE CASCADE), Sicherung und
-- Benutzertrennung wie bei allen anderen Daten funktionieren.
-- next_try_at: Unix-Sekunden, frühester Zeitpunkt für den nächsten Versuch.

CREATE TABLE article_images (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    article_id  INTEGER NOT NULL REFERENCES articles (id) ON DELETE CASCADE,
    url         TEXT    NOT NULL,
    status      TEXT    NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending', 'working', 'done', 'failed')),
    attempts    INTEGER NOT NULL DEFAULT 0,
    next_try_at INTEGER NOT NULL DEFAULT 0,
    mime        TEXT,
    width       INTEGER,
    height      INTEGER,
    data        BLOB,
    created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE (article_id, url)
);
CREATE INDEX idx_article_images_queue ON article_images (status, next_try_at);
