-- Migration 0003: Volltextsuche (SQLite FTS5) über Titel und Artikeltext.
-- Der Index verweist auf die Tabelle `articles` (external content) und wird per
-- Trigger aktuell gehalten. Umlaute und Akzente werden beim Suchen gefaltet.

CREATE VIRTUAL TABLE articles_fts USING fts5(
    title,
    content_text,
    content = 'articles',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

-- Bereits gespeicherte Artikel in den Index aufnehmen.
INSERT INTO articles_fts (articles_fts) VALUES ('rebuild');

CREATE TRIGGER articles_fts_insert AFTER INSERT ON articles BEGIN
    INSERT INTO articles_fts (rowid, title, content_text)
    VALUES (new.id, new.title, new.content_text);
END;

CREATE TRIGGER articles_fts_delete AFTER DELETE ON articles BEGIN
    INSERT INTO articles_fts (articles_fts, rowid, title, content_text)
    VALUES ('delete', old.id, old.title, old.content_text);
END;

CREATE TRIGGER articles_fts_update AFTER UPDATE OF title, content_text ON articles BEGIN
    INSERT INTO articles_fts (articles_fts, rowid, title, content_text)
    VALUES ('delete', old.id, old.title, old.content_text);
    INSERT INTO articles_fts (rowid, title, content_text)
    VALUES (new.id, new.title, new.content_text);
END;

-- Filter nach Gelesen-Status in der Liste.
CREATE INDEX idx_articles_user_read ON articles (user_id, is_read, id DESC);
