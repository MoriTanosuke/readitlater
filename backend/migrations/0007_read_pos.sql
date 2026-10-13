-- Migration 0007: Leseposition je Artikel.
-- read_pos ist ein Textoffset (UTF-16-Einheiten über den dargestellten Artikeltext, dieselbe
-- Zählung wie bei den Markierungen), keine Pixelzahl: Damit stimmt die Stelle auch dann, wenn
-- Schrift, Breite oder Gerät wechseln. 0 heißt „am Anfang“ (auch: gelesen oder bis zum Ende
-- gescrollt). Das Backend erzwingt: Ein gelesener Artikel hat immer read_pos = 0.

ALTER TABLE articles ADD COLUMN read_pos INTEGER NOT NULL DEFAULT 0
    CHECK (read_pos >= 0);
