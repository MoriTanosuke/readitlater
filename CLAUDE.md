# Leseliste (Read-it-later-App)

Artikel per URL speichern, Volltext durchsuchen, mit Schlagwörtern versehen und als gelesen markieren. Mehrere Benutzer, jeder sieht nur seine eigenen Daten.

## Stack

- **Backend** (`/backend`): Rust, axum, tokio, sqlx (SQLite, eingebettete Migrationen), argon2, eigene Session-Verwaltung
- **Frontend** (`/frontend/public`): nur HTML, CSS und JavaScript (ES-Module), keine Buildchain, kein npm
- **Auslieferung**: Caddy (`/frontend/Caddyfile`) liefert das Frontend aus und leitet `/api/*` ans Backend. Dadurch gibt es nur eine Origin und kein CORS.
- **Deployment**: GitHub Actions baut zwei Images nach ghcr.io (`<repo>-backend`, `<repo>-web`), Watchtower (Fork `nicholas-fedor/watchtower`) zieht sie auf dem Server. Der Push auf `main` deployt automatisch.
- **Daten**: SQLite im WAL-Modus, Datei liegt im Volume `/data`, nie im Image.

## Befehle

```sh
cd backend
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test   # vor jedem Push
DATABASE_PATH=./data/app.db cargo run                                   # Backend lokal auf :3000

# Komplettes System lokal (Caddy + Backend), http://localhost:8080
docker compose -f docker-compose.yml -f docker-compose.dev.yml up --build
```

Konfiguration nur über Umgebungsvariablen: `DATABASE_PATH`, `BIND_ADDR`, `REGISTRATION_ENABLED`, `COOKIE_SECURE`, `SESSION_DAYS`, `RUST_LOG`.

## Arbeitsweise

- Änderungen immer auf einem Branch mit Pull Request, nie direkt auf `main`.
- Schemaänderungen nur als neue Migration in `backend/migrations`, bestehende Migrationen nie verändern.
- Neue Abhängigkeiten sparsam einführen und kurz begründen.
- Antworten und Code-Kommentare auf Deutsch, Bezeichner auf Englisch.
- Wichtige Entscheidungen hier festhalten.

## Entscheidungen

- **Sessions**: zufälliges Token (32 Byte) im Cookie `session` (HttpOnly, SameSite=Lax, optional Secure). In der Datenbank steht nur der SHA-256-Hash.
- **Passwörter**: argon2id (Standardparameter), mindestens 10 Zeichen. Login prüft auch bei unbekannter E-Mail einen Hash und gibt immer dieselbe Fehlermeldung zurück.
- **CSRF**: Alle Anfragen außer GET/HEAD/OPTIONS brauchen den Header `X-Requested-With`. Das Frontend setzt ihn in `js/api.js`.
- **Konto löschen**: erfordert das Passwort. Artikel, Schlagwörter und Sessions verschwinden per `ON DELETE CASCADE` (Fremdschlüssel sind pro Verbindung aktiviert, siehe `db.rs`).
- **Registrierung**: per `REGISTRATION_ENABLED=false` abschaltbar.
- **Frontend-Regel**: nie `innerHTML` mit Daten aus der API. Immer `textContent` oder DOM-Methoden. Die CSP in der Caddyfile erlaubt keine Inline-Skripte.
- **Versionen** (Stand Okt. 2026): sqlx 0.9 akzeptiert nur `&'static str` als SQL (dynamische Strings nur mit `AssertSqlSafe`), argon2 0.6 nutzt `password_hash::phc::PasswordHash` und erzeugt den Salt selbst.

## Status und Planung

1. Fertig: Grundgerüst, Migration 0001, Registrierung, Login, Logout, Konto löschen, Caddy, Dockerfiles, Compose, Workflow, Integrationstests
2. Offen: Artikel speichern (URL laden, Hauptinhalt extrahieren, SSRF-Schutz mit Blockierung von localhost und privaten IP-Bereichen auch nach Redirects, Timeout, Größenlimit), Artikel löschen
3. Offen: Schlagwörter, Gelesen-Status, Suche mit SQLite FTS5 (Trigger für Insert, Update, Delete)
4. Offen: Frontend ausbauen (Liste, Lesemodus, Suche, mobil optimiert), gespeicherten Inhalt mit `ammonia` bereinigen
5. Offen: Ratenbegrenzung für Login und Registrierung, bevor der Server aus dem Internet erreichbar ist
