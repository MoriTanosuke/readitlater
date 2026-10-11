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

**Ratenbegrenzung:** Das Backend bremst Anmeldeversuche, Registrierungen und Seitenabrufe pro IP, Konto und Benutzer und antwortet dann mit `429` und `Retry-After`. Sie ist standardmäßig an (`RATE_LIMIT_ENABLED=false` schaltet sie ab). Die Client-Adresse kommt aus `X-Forwarded-For` von Caddy, deshalb darf das Backend nur über Caddy erreichbar sein (in der Compose-Datei ist sein Port nicht veröffentlicht).

**Registrierung:** Nach dem Anlegen der eigenen Konten `REGISTRATION_ENABLED=false` in `.env` setzen und `docker compose up -d` ausführen.

## Artikel aus anderen Apps teilen

Im Benutzermenü (E-Mail oben rechts) gibt es „Teilen aus Apps“. Dort legst du pro Gerät ein Token an (wird nur einmal angezeigt, es lässt sich einzeln widerrufen). Ein Token darf ausschließlich Artikel hinzufügen. Das Gerät muss den Server erreichen können, bei rein internem Betrieb also im Heimnetz, per VPN oder über Tailscale.

```sh
# Test von der Kommandozeile
curl -X POST https://<server>/api/share \
  -H "Authorization: Bearer lsl_..." \
  -H "Content-Type: application/json" \
  -d '{"url": "https://example.org/artikel"}'
```

Die Schnittstelle akzeptiert die Adresse als JSON (`url` oder `text`), als Formular (`url` oder `text`) oder als reinen Text im Body. Steht im Text mehr als die Adresse (so teilt Android oft: „Titel https://…“), zählt die erste Web-Adresse. Antworten: `201` mit Titel und ID, `409` wenn der Artikel schon gespeichert ist, `400` bei ungültiger Adresse, `401` bei fehlendem oder widerrufenem Token.

- **Android (HTTP Shortcuts):** Shortcut mit Methode `POST`, URL `https://<server>/api/share`, Header `Authorization: Bearer <Token>`, Body-Typ „Benutzerdefinierter Text“ (`text/plain`) mit der Variable für den geteilten Text. Die Variable muss den Wert aus dem Teilen-Dialog übernehmen, und der Shortcut muss im Teilen-Menü erscheinen. Die Bezeichnungen der Einstellungen unterscheiden sich je nach Version der App.
- **iPad und iPhone (Kurzbefehle):** Neuer Kurzbefehl mit „Im Share Sheet anzeigen“ (Eingabe: URLs und Text), Aktion „Inhalte von URL abrufen“ mit der Schnittstellen-Adresse, Methode `POST`, Header `Authorization: Bearer <Token>` und als Anfragetext die „Kurzbefehleingabe“.

## Markierungen und Notizen

Im Lesemodus lässt sich Text auswählen und mit einer von fünf Farben oder einer eigenen Farbe markieren. Ein Klick auf eine Markierung öffnet das Bedienfeld: Farbe ändern, optionale Notiz schreiben, Markierung löschen. Notizen stehen neben dem Text (auf schmalen Bildschirmen darunter). „Markierungen ausblenden“ zeigt den Artikel ohne Farben und Notizen.

## Installieren und offline lesen (PWA)

Die Leseliste lässt sich wie eine App installieren („Zum Startbildschirm hinzufügen“ bzw. „App installieren“) und gespeicherte Artikel auch ohne Internet lesen, zum Beispiel im Zug.

**Wichtig: Das funktioniert nur über HTTPS.** Browser erlauben den dafür nötigen Service Worker nur auf `localhost` und auf HTTPS-Adressen. Mit der Standardeinstellung (HTTP auf Port 80, z. B. im Heimnetz) läuft die App normal, aber ohne Installation und ohne Start ohne Netz. Setze dafür `SITE_ADDRESS` auf deine Domain (Caddy holt das Zertifikat selbst, siehe oben) und `COOKIE_SECURE=true`. Hinter Tailscale geht es mit `tailscale serve` und dem HTTPS-Namen des Geräts.

- Das Gerät speichert die 30 neuesten ungelesenen Artikel samt Markierungen und Notizen und jeden Artikel, den du geöffnet hast (14 Tage, höchstens 30). Der Abgleich läuft beim Öffnen der Liste im Hintergrund, unter „Offline-Lesen“ im Konto-Menü lässt er sich auch von Hand starten, ausschalten oder löschen.
- Ohne Verbindung (oder bei sehr schlechter) siehst du die gespeicherten Artikel, kannst sie lesen, durchsuchen und als gelesen markieren. Das Markieren wird nachgeholt, sobald wieder Verbindung besteht. Neue Artikel speichern, Schlagwörter, Markierungen und Notizen ändern braucht eine Verbindung.
- Die Artikel liegen nur in diesem Browser. Beim Abmelden werden sie vom Gerät gelöscht. Abmelden geht deshalb nur mit Verbindung.
- Android: In der installierten App erscheint die Leseliste im Teilen-Menü anderer Apps. Das Teilen speichert den Artikel nach der Anmeldung. iPhone und iPad unterstützen das nicht, dort bleibt der Kurzbefehl (siehe oben).

## Export und Import

Im Konto-Menü unter „Export und Import“ lädt ein Klick auf „Alle Artikel herunterladen“ eine einzelne JSON-Datei mit den eigenen Artikeln (Adresse, Titel, Inhalt, Schlagwörter, Gelesen-Status). Es braucht nur den Browser, keine Entpackprogramme. Die Datei enthält keine Kontodaten.

Markierungen und Notizen gehören zu den Artikeln und sind in der Datei enthalten.

Importiert werden nur Dateien, die diese App selbst exportiert hat (maximal 64 MiB, 20.000 Artikel). Beim Import wird nichts aus dem Internet geladen, der Inhalt wird neu bereinigt, und bereits vorhandene Artikel (gleiche Adresse) bleiben unverändert. Es gibt keine Kompatibilität mit anderen Diensten.

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
