-- Migration 0006: Sprache des Artikels (ISO 639-1, z. B. "de"), erkannt aus dem Text.
-- NULL heißt unbekannt (ältere Artikel oder unsichere Erkennung). Das Frontend setzt damit
-- das lang-Attribut des Artikeltextes, was Silbentrennung und Screenreader nutzen.

ALTER TABLE articles ADD COLUMN lang TEXT
    CHECK (lang IS NULL OR (length(lang) BETWEEN 2 AND 3 AND lang = lower(lang)));
