# Leseliste (Read-it-later-App)

Artikel per URL speichern, Volltext durchsuchen, mit Schlagwörtern versehen und als gelesen markieren. Mehrere Benutzer, jeder sieht nur seine eigenen Daten.

## Stack

- **Backend** (`/backend`): Rust, axum, tokio, sqlx (SQLite, eingebettete Migrationen), argon2, eigene Session-Verwaltung
- **Frontend** (`/frontend/public`): nur HTML, CSS und JavaScript (ES-Module), keine Buildchain, kein npm. Einzige Ausnahme sind die Tests in `/frontend/tests` (npm mit `jsdom` als einziger Entwicklungsabhängigkeit), sie gehören nicht in die Images.
- **Auslieferung**: Caddy (`/frontend/Caddyfile`) liefert das Frontend aus und leitet `/api/*` ans Backend. Dadurch gibt es nur eine Origin und kein CORS.
- **Deployment**: GitHub Actions baut zwei Images nach ghcr.io (`<repo>-backend`, `<repo>-web`), Watchtower (Fork `nicholas-fedor/watchtower`) zieht sie auf dem Server. Der Push auf `main` deployt automatisch.
- **Daten**: SQLite im WAL-Modus, Datei liegt im Volume `/data`, nie im Image.

## Befehle

```sh
cd backend
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test   # vor jedem Push
DATABASE_PATH=./data/app.db cargo run                                   # Backend lokal auf :3000

# Komplettes System lokal (Caddy + Backend), http://localhost:8080
# Frontend-Tests (echtes Frontend in jsdom gegen das gebaute Backend; Node 22)
cd backend && cargo build && cd ../frontend/tests && npm ci && npm test

docker compose -f docker-compose.yml -f docker-compose.dev.yml up --build
```

Konfiguration nur über Umgebungsvariablen: `DATABASE_PATH`, `BIND_ADDR`, `REGISTRATION_ENABLED`, `COOKIE_SECURE`, `SESSION_DAYS`, `RUST_LOG`, `FETCH_ALLOW_PRIVATE` (Standard false, schaltet den SSRF-Schutz ab, nur für Tests/vertrauenswürdige Netze), `FETCH_TIMEOUT_SECS` (15), `FETCH_MAX_BYTES` (2 MiB), `FETCH_CONCURRENCY` (4).

## Arbeitsweise

- Änderungen immer auf einem Branch mit Pull Request, nie direkt auf `main`.
- Schemaänderungen nur als neue Migration in `backend/migrations`, bestehende Migrationen nie verändern.
- Neue Abhängigkeiten sparsam einführen und kurz begründen.
- Antworten und Code-Kommentare auf Deutsch, Bezeichner auf Englisch.
- Wichtige Entscheidungen hier festhalten.

## Entscheidungen

- **Tests**: Rust-Tests (Unit und Integration) liegen in `backend`. Die Frontend-Tests in `frontend/tests` starten das echte Backend mit frischer temporärer Datenbank und einen lokalen Testserver für die Artikelseiten und laden `index.html` samt `app.js` in jsdom (`node --test`). `FETCH_ALLOW_PRIVATE=true` ist dafür gesetzt. Die CI führt beides im Job `test` aus, die Image-Builds laufen erst danach (`needs: test`). Ein anderes Backend-Programm lässt sich mit `BACKEND_BIN` angeben.

- **Sessions**: zufälliges Token (32 Byte) im Cookie `session` (HttpOnly, SameSite=Lax, optional Secure). In der Datenbank steht nur der SHA-256-Hash.
- **Passwörter**: argon2id (Standardparameter), mindestens 10 Zeichen. Login prüft auch bei unbekannter E-Mail einen Hash und gibt immer dieselbe Fehlermeldung zurück.
- **CSRF**: Alle Anfragen außer GET/HEAD/OPTIONS brauchen den Header `X-Requested-With`. Das Frontend setzt ihn in `js/api.js`.
- **Passwort ändern**: `PUT /api/account/password` mit bisherigem und neuem Passwort (gleiche Regeln wie bei der Registrierung). Danach werden alle Sessions des Benutzers gelöscht und für das aktuelle Gerät wird eine neue angelegt.
- **Suche**: SQLite FTS5 (Migration 0003) als External-Content-Index `articles_fts` über `title` und `content_text`, per Trigger für Insert, Update und Delete aktuell. Tokenizer `unicode61 remove_diacritics 2` (Umlaute werden gefaltet). Der Suchtext wird nie roh an FTS5 gegeben: jedes Wort wird als Phrase in Anführungszeichen gesetzt, das letzte als Präfix, alle müssen vorkommen (`search_query` in `articles.rs`). Sortierung nach Relevanz (bm25), Treffer im Auszug stehen zwischen den Steuerzeichen U+0001 und U+0002 und werden im Frontend per DOM als `<mark>` gesetzt.
- **Schlagwörter und Gelesen-Status**: `PATCH /api/articles/{id}` mit `is_read` und/oder `tags` (Liste ersetzt die bisherigen). Schlagwörter sind pro Benutzer, ohne Beachtung der Groß-/Kleinschreibung eindeutig, max. 40 Zeichen, max. 20 pro Artikel, keine Kommas. Nicht mehr verwendete Schlagwörter werden automatisch gelöscht. `GET /api/tags` liefert Name und Anzahl, `GET /api/articles` filtert mit `q`, `tag` und `read`.
- **Konto-Menü**: Die E-Mail oben rechts öffnet ein Menü (Disclosure-Muster mit `aria-expanded`, schließt bei Escape, Klick daneben und Navigation). Passwort ändern und Konto löschen haben eigene Seiten unter `#/account/password` und `#/account/delete` mit „Abbrechen“ zurück zur Liste. Nach erfolgreicher Passwortänderung zeigt die Liste einmalig einen Hinweis. Die Backend-API ist unverändert.
- **Konto löschen**: erfordert das Passwort. Artikel, Schlagwörter und Sessions verschwinden per `ON DELETE CASCADE` (Fremdschlüssel sind pro Verbindung aktiviert, siehe `db.rs`).
- **Registrierung**: per `REGISTRATION_ENABLED=false` abschaltbar.
- **Artikel abrufen (SSRF-Schutz)**: nur http/https ohne Zugangsdaten; ein eigener DNS-Resolver (`PublicOnlyResolver` in `fetch.rs`) liefert nur öffentliche IPs, das gilt damit auch für Redirects (max. 5, jeder Hop wird geprüft) und gegen DNS-Rebinding. Kein Proxy, Gesamt-Timeout, Größenlimit auf die entpackten Bytes (Gzip-Bomben), nur HTML, gleichzeitige Abrufe begrenzt (sonst 429). Duplikate (pro Benutzer, ohne Fragment) liefern 409 ohne erneuten Abruf.
- **Extraktion**: `dom_smoothie` (Readability), Ergebnis wird mit `ammonia` bereinigt. `content` = bereinigtes HTML, `content_text` = Klartext (für FTS5 in Schritt 3), `excerpt` (Migration 0002). TLS über rustls mit `ring` (einfacher für musl-Builds als aws-lc-rs).
- **Frontend-Regel**: Ausnahme ist `js/safe-html.js`: Artikel-HTML wird per DOMParser geparst und mit createElement über eine Positivliste neu aufgebaut (zusätzlich zu ammonia). Die CSP erlaubt `img-src 'self' https: data:`.
- **Frontend-Regel**: nie `innerHTML` mit Daten aus der API. Immer `textContent` oder DOM-Methoden. Die CSP in der Caddyfile erlaubt keine Inline-Skripte.
- **Versionen** (Stand Okt. 2026): sqlx 0.9 akzeptiert nur `&'static str` als SQL (dynamische Strings nur mit `AssertSqlSafe`), argon2 0.6 nutzt `password_hash::phc::PasswordHash` und erzeugt den Salt selbst.

## Status und Planung

1. Fertig: Grundgerüst, Migration 0001, Registrierung, Login, Logout, Konto löschen, Caddy, Dockerfiles, Compose, Workflow, Integrationstests
2. Fertig: Artikel speichern (URL laden, Hauptinhalt extrahieren, SSRF-Schutz mit Blockierung von localhost und privaten IP-Bereichen auch nach Redirects, Timeout, Größenlimit), Artikel löschen
3. Fertig: Schlagwörter, Gelesen-Status, Suche mit SQLite FTS5 (Trigger für Insert, Update, Delete)
4. Fertig: Liste, Lesemodus, Suche, Filter (gelesen, Schlagwort), Schlagwörter bearbeiten, mobil optimiert, Bereinigung mit `ammonia`. Noch offen: Schlagwörter umbenennen/global löschen, Import/Export
5. Offen: Ratenbegrenzung für Login und Registrierung, bevor der Server aus dem Internet erreichbar ist
