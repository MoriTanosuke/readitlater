# Leseliste (Read-it-later-App)

Artikel per URL speichern, Volltext durchsuchen, mit Schlagwörtern versehen und als gelesen markieren. Mehrere Benutzer, jeder sieht nur seine eigenen Daten.

## Stack

- **Backend** (`/backend`): Rust, axum, tokio, sqlx (SQLite, eingebettete Migrationen), argon2, eigene Session-Verwaltung
- **Frontend** (`/frontend/public`): nur HTML, CSS und JavaScript (ES-Module). Der Code bleibt statisch: keine npm-Pakete, keine Buildchain, kein Transpiler oder Bundler. Npm ist nur für Testwerkzeuge in `/frontend/tests` erlaubt (siehe Entscheidung „Tests“), sie gehören nie in die Images.
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

Konfiguration nur über Umgebungsvariablen: `DATABASE_PATH`, `BIND_ADDR`, `REGISTRATION_ENABLED`, `COOKIE_SECURE`, `SESSION_DAYS`, `RUST_LOG`, `FETCH_ALLOW_PRIVATE` (Standard false, schaltet den SSRF-Schutz ab, nur für Tests/vertrauenswürdige Netze), `FETCH_TIMEOUT_SECS` (15), `FETCH_MAX_BYTES` (2 MiB), `FETCH_CONCURRENCY` (4), `RATE_LIMIT_ENABLED` (Standard true).

## Arbeitsweise

- Änderungen immer auf einem Branch mit Pull Request, nie direkt auf `main`.
- Schemaänderungen nur als neue Migration in `backend/migrations`, bestehende Migrationen nie verändern.
- Neue Abhängigkeiten sparsam einführen und kurz begründen.
- Antworten und Code-Kommentare auf Deutsch, Bezeichner auf Englisch.
- Wichtige Entscheidungen hier festhalten.

## Entscheidungen

- **Tests**: Rust-Tests (Unit und Integration) liegen in `backend`. Die Frontend-Tests in `frontend/tests` starten das echte Backend mit frischer temporärer Datenbank und einen lokalen Testserver für die Artikelseiten und laden `index.html` samt `app.js` in jsdom (`node --test`). `FETCH_ALLOW_PRIVATE=true` ist dafür gesetzt. Die CI führt beides im Job `test` aus, die Image-Builds laufen erst danach (`needs: test`). Ein anderes Backend-Programm lässt sich mit `BACKEND_BIN` angeben. **Regel (vom Betreiber festgelegt):** Das Vorgehen gilt nur, solange das Frontend statisch bleibt, ohne npm-Pakete und ohne Buildchain für den Frontend-Code. Als Testwerkzeug ist jsdom erlaubt, für Smoketests gegen die laufenden Container auch Playwright. Weitere Testabhängigkeiten nur sparsam und mit kurzer Begründung.

- **Sessions**: zufälliges Token (32 Byte) im Cookie `session` (HttpOnly, SameSite=Lax, optional Secure). In der Datenbank steht nur der SHA-256-Hash.
- **Passwörter**: argon2id (Standardparameter), mindestens 10 Zeichen. Login prüft auch bei unbekannter E-Mail einen Hash und gibt immer dieselbe Fehlermeldung zurück.
- **CSRF**: Alle Anfragen außer GET/HEAD/OPTIONS brauchen den Header `X-Requested-With`. Das Frontend setzt ihn in `js/api.js`.
- **Passwort ändern**: `PUT /api/account/password` mit bisherigem und neuem Passwort (gleiche Regeln wie bei der Registrierung). Danach werden alle Sessions des Benutzers gelöscht und für das aktuelle Gerät wird eine neue angelegt.
- **Suche**: SQLite FTS5 (Migration 0003) als External-Content-Index `articles_fts` über `title` und `content_text`, per Trigger für Insert, Update und Delete aktuell. Tokenizer `unicode61 remove_diacritics 2` (Umlaute werden gefaltet). Der Suchtext wird nie roh an FTS5 gegeben: jedes Wort wird als Phrase in Anführungszeichen gesetzt, das letzte als Präfix, alle müssen vorkommen (`search_query` in `articles.rs`). Sortierung nach Relevanz (bm25), Treffer im Auszug stehen zwischen den Steuerzeichen U+0001 und U+0002 und werden im Frontend per DOM als `<mark>` gesetzt.
- **Schlagwörter und Gelesen-Status**: `PATCH /api/articles/{id}` mit `is_read` und/oder `tags` (Liste ersetzt die bisherigen). Schlagwörter sind pro Benutzer, ohne Beachtung der Groß-/Kleinschreibung eindeutig, max. 40 Zeichen, max. 20 pro Artikel, keine Kommas. Nicht mehr verwendete Schlagwörter werden automatisch gelöscht. `GET /api/tags` liefert Name und Anzahl, `GET /api/articles` filtert mit `q`, `tag` und `read`.
- **Konto-Menü**: Die E-Mail oben rechts öffnet ein Menü (Disclosure-Muster mit `aria-expanded`, schließt bei Escape, Klick daneben und Navigation). Passwort ändern und Konto löschen haben eigene Seiten unter `#/account/password` und `#/account/delete` mit „Abbrechen“ zurück zur Liste. Nach erfolgreicher Passwortänderung zeigt die Liste einmalig einen Hinweis. Die Backend-API ist unverändert.
- **Konto löschen**: erfordert das Passwort. Artikel, Schlagwörter und Sessions verschwinden per `ON DELETE CASCADE` (Fremdschlüssel sind pro Verbindung aktiviert, siehe `db.rs`).
- **Teilen aus anderen Apps**: `POST /api/share` mit `Authorization: Bearer <Token>` (Android HTTP Shortcuts, iOS Kurzbefehle). Tokens (Migration 0004, `share_tokens`) werden in der Weboberfläche unter `#/account/share` angelegt (`/api/share-tokens`, nur per Session, max. 10 pro Benutzer), haben das Präfix `lsl_`, 32 Byte Zufall, in der Datenbank nur der SHA-256-Hash. Ein Token gilt **nur** für `/api/share` (Least Privilege: kann Artikel hinzufügen, nichts lesen oder löschen) und lässt sich einzeln widerrufen. `/api/share` liegt außerhalb des CSRF-Guards, weil es nur den Header `Authorization` auswertet und kein Cookie. Der Body darf JSON, Formular oder reiner Text sein, die erste http(s)-Adresse im Text zählt (Android teilt „Titel Adresse“). Gespeichert wird über `articles::save_article`, es gelten SSRF-Schutz, Duplikatprüfung und Abruf-Limit wie bei `POST /api/articles`. Eine Passwortänderung widerruft Tokens nicht, sie sind eigene Zugangsdaten.
- **Ratenbegrenzung** (`ratelimit.rs`, Crate `governor`, nur Speicher, bewusst nicht in SQLite: jede Anfrage wäre sonst ein Schreibzugriff auf den einzigen Schreiber): Stufen mit Token-Bucket pro Schlüssel. `general` (120 sofort, danach 2/s pro IP, alle Routen außer `/health`, äußerste Middleware), `auth` (10 sofort, danach 1 alle 6 s pro IP, Login, Registrierung, Passwort ändern, Konto löschen), `login_pair` (IP und E-Mail: 5, danach 1/min) und `login_email` (E-Mail allein: 30, danach 1 alle 2 min, bremst verteilte Angriffe), `password_user` (pro Benutzer bei Passwort ändern und Konto löschen: 5, danach 1/min), `fetch_user` (pro Benutzer: 10, danach 1 alle 6 s) in `articles::save_article`, damit Speichern und Teilen dasselbe Kontingent teilen. Duplikate zählen nicht. Antwort 429 mit `Retry-After`. Die Prüfung steht vor Body-Parsing, Anmeldung und Hashing. Weil die IP-Stufen vorher greifen, ist die Zahl der E-Mail-Schlüssel begrenzt. Ein Hintergrundjob entfernt jede Minute erholte Einträge. Client-IP: letzter Eintrag von `X-Forwarded-For` (den hängt Caddy an, vom Client mitgeschickte Werte stehen davor), sonst die Adresse der Verbindung; IPv6 zählt als /64. Das Backend darf deshalb nie direkt aus dem Internet erreichbar sein (nur über Caddy). Grenzen des Ansatzes: Ein Neustart setzt die Zähler zurück; `login_email` kann ein Konto für kurze Zeit für alle bremsen (bewusst, statt harter Sperre). Die Tests schalten die Begrenzung ab (`RATE_LIMIT_ENABLED=false`), außer `tests/ratelimit.rs`.
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
4a. Fertig: Teilen aus Android und iPad per Token (`POST /api/share`, Verwaltung unter `#/account/share`)
5. Fertig: Ratenbegrenzung (siehe Entscheidung „Ratenbegrenzung“). Weitere Punkte vor dem Internetbetrieb stehen im Security-Audit (HTTPS, Tiefenlimit für HTML)
