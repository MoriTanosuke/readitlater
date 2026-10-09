-- Migration 0002: Artikeltext für die Suche (Schritt 3) und Kurzvorschau für die Liste.
-- `content` enthält ab jetzt bereinigtes HTML (ammonia), `content_text` den reinen Text.

ALTER TABLE articles ADD COLUMN content_text TEXT NOT NULL DEFAULT '';
ALTER TABLE articles ADD COLUMN excerpt TEXT NOT NULL DEFAULT '';
