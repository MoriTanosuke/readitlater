# Leseliste

Read-it-later-App mit Rust-Backend (axum, SQLite) und Frontend aus reinem HTML, CSS und JavaScript. Details zu Stack und Entscheidungen stehen in `CLAUDE.md`.

## Ablauf

1. Push auf `main` startet den Workflow `.github/workflows/build.yml`. Er prüft das Backend (fmt, clippy, Tests) und das Frontend (jsdom-Tests gegen das gebaute Backend) und baut zwei Images nach ghcr.io: `ghcr.io/<benutzer>/<repo>-backend` und `...-web`.
2. Auf dem Server holt Watchtower neue Images (Prüfintervall 5 Minuten) und startet die Container neu.
3. In Pull Requests werden Tests und Image-Build ausgeführt, aber nichts veröffentlicht.

## Einrichtung auf dem Server (einmalig)

```sh
mkdir readitlater && cd readitlater
# docker-compose.yml und .env.example aus dem Repository hierher kopieren
cp .env.example .env      # GITHUB_REPO=benutzer/repo (Kleinbuchstaben) eintragen
docker compose up -d
```

Die App läuft dann auf `http://<server>:8080`.

**Images in ghcr.io:** Pakete aus privaten Repositories sind privat. Entweder das Paket in den GitHub-Paketeinstellungen auf „public“ stellen oder auf dem Server `docker login ghcr.io` (Token mit `read:packages`) ausführen und in der Compose-Datei die auskommentierte `config.json`-Zeile bei Watchtower aktivieren.

**Watchtower:** Das Original `containrrr/watchtower` ist seit Dezember 2025 archiviert. Die Compose-Datei nutzt den Fork `ghcr.io/nicholas-fedor/watchtower`. Prüfe nach dem ersten Start mit `docker compose logs watchtower`, ob er die Label-Filterung und das Polling wie erwartet übernimmt.

**HTTPS und Cookies:** Im Heimnetz oder hinter Tailscale reicht HTTP. Bei Zugriff über eine Domain `SITE_ADDRESS=domain` setzen (Caddy holt dann ein Zertifikat, Port 80 und 443 müssen erreichbar sein, 443 in der Compose-Datei ergänzen) und `COOKIE_SECURE=true` setzen.

**Registrierung:** Nach dem Anlegen der eigenen Konten `REGISTRATION_ENABLED=false` in `.env` setzen und `docker compose up -d` ausführen.

## Datensicherung

Die SQLite-Datei liegt im Volume `readitlater_app-data`. Konsistente Sicherung bei laufender App:

```sh
docker run --rm -v readitlater_app-data:/data -v "$PWD":/backup alpine \
  sh -c 'apk add --no-cache sqlite >/dev/null && sqlite3 /data/app.db ".backup /backup/app-$(date +%F).db"'
```

## Lokal ausprobieren

```sh
docker compose -f docker-compose.yml -f docker-compose.dev.yml up --build   # http://localhost:8080
# oder nur das Backend:
cd backend && cargo test && cargo run
```
