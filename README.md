# Leseliste

Read-it-later-App mit Rust-Backend (axum, SQLite) und Frontend aus reinem HTML, CSS und JavaScript. Details zu Stack und Entscheidungen stehen in `CLAUDE.md`.

## Ablauf

1. Push auf `main` startet den Workflow `.github/workflows/build.yml`. Er prüft das Backend (fmt, clippy, Tests) und das Frontend (jsdom-Tests gegen das gebaute Backend) und baut ein Image nach ghcr.io: `ghcr.io/<benutzer>/<repo>`. Das Frontend steckt im Binary, es gibt nur diesen einen Container.
2. Auf dem Server holt Watchtower neue Images (Prüfintervall 5 Minuten) und startet die Container neu.
3. In Pull Requests werden Tests und Image-Build ausgeführt, aber nichts veröffentlicht.

## Einrichtung auf dem Server (einmalig)

```sh
mkdir readitlater && cd readitlater
# docker-compose.yml und .env.example aus dem Repository hierher kopieren
cp .env.example .env      # GITHUB_REPO=benutzer/repo (Kleinbuchstaben) eintragen
docker compose up -d
```

Der Container veröffentlicht keinen Port. Er hängt im Docker-Netz deines vorhandenen Caddy (`PROXY_NETWORK` in `.env`, Netz muss existieren). Im Caddyfile genügt:

```
lesen.example.org {
	encode zstd gzip
	reverse_proxy app:3000
}
```

(Läuft Caddy nicht im selben Netz, den Containernamen bzw. die Adresse anpassen.) Caddy kümmert sich um HTTPS und Komprimierung, die Sicherheits-Header setzt die App selbst.

**Images in ghcr.io:** Pakete aus privaten Repositories sind privat. Entweder das Paket in den GitHub-Paketeinstellungen auf „public“ stellen oder auf dem Server `docker login ghcr.io` (Token mit `read:packages`) ausführen und in der Compose-Datei die auskommentierte `config.json`-Zeile bei Watchtower aktivieren.

**Watchtower:** Das Original `containrrr/watchtower` ist seit Dezember 2025 archiviert. Die Compose-Datei nutzt den Fork `ghcr.io/nicholas-fedor/watchtower`. Prüfe nach dem ersten Start mit `docker compose logs watchtower`, ob er die Label-Filterung und das Polling wie erwartet übernimmt.

**HTTPS und Cookies:** `COOKIE_SECURE` steht standardmäßig auf `true` (Caddy terminiert HTTPS). Nur ohne HTTPS (reiner Test im Heimnetz) auf `false` setzen.

**Umstieg vom Setup mit zwei Containern:** Die Dienste `backend` und `web` gibt es nicht mehr, die alten Images `...-backend` und `...-web` werden nicht mehr gebaut. Neue `docker-compose.yml` und `.env.example` übernehmen (`WEB_PORT` und `SITE_ADDRESS` entfallen), `docker compose up -d --remove-orphans` ausführen und im Caddy auf `reverse_proxy app:3000` umstellen. Compose-Projektname und Volume bleiben gleich, die Daten sind also weiter da. Zur Sicherheit vorher sichern (siehe Datensicherung).

**Ratenbegrenzung:** Das Backend bremst Anmeldeversuche, Registrierungen und Seitenabrufe pro IP, Konto und Benutzer und antwortet dann mit `429` und `Retry-After`. Sie ist standardmäßig an (`RATE_LIMIT_ENABLED=false` schaltet sie ab). Die Client-Adresse kommt aus `X-Forwarded-For` vom vorgeschalteten Caddy, deshalb darf das Backend nur über ihn erreichbar sein (in der Compose-Datei ist kein Port veröffentlicht).

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

## Darstellung anpassen

Im Lesemodus öffnet der Knopf „Aa“ ein Bedienfeld, im Konto-Menü gibt es zusätzlich die Seite „Darstellung“ mit Vorschau. Einstellbar sind Schriftart (Systemschrift, Serif, breite Schrift, Monospace), Textgröße, Textbreite (Zeichen pro Zeile), Zeilenabstand, Textausrichtung (linksbündig oder Blocksatz mit automatischer Silbentrennung in der Sprache des Artikels, die beim Speichern aus dem Text erkannt wird; ältere Artikel gelten als deutsch), Farbschema (wie das Gerät, hell oder dunkel) und Kontrast (weich, normal, hoch, jeweils für Hell und Dunkel). Die Einstellungen gelten pro Gerät und Browser und funktionieren auch offline. „Zurücksetzen“ stellt die Voreinstellung her. Dafür braucht der Browser `light-dark()` (Chrome ab 123, Safari ab 17.5, Firefox ab 120).

## Markierungen und Notizen

Im Lesemodus lässt sich Text auswählen und mit einer von fünf Farben oder einer eigenen Farbe markieren. Ein Klick auf eine Markierung öffnet das Bedienfeld: Farbe ändern, optionale Notiz schreiben, Markierung löschen. Notizen stehen neben dem Text (auf schmalen Bildschirmen darunter). „Markierungen ausblenden“ zeigt den Artikel ohne Farben und Notizen.

## Lesefilter

Die Auswahl Alle/Ungelesen/Gelesen über der Liste merkt sich das Gerät. Sie bleibt nach dem Speichern eines Artikels, beim Neuladen und nach erneutem Anmelden erhalten. Nur Suchtext und Schlagwort werden nach dem Speichern zurückgesetzt.

## Leseposition

Die Leseliste merkt sich still, wo du in einem ungelesenen Artikel stehst, und springt beim erneuten Öffnen dorthin zurück, auch auf einem anderen Gerät oder mit anderer Schriftgröße. Es gibt dafür keine Bedienung und keine Anzeige. Der Artikel beginnt wieder oben, wenn du ihn als gelesen markierst oder bis zum Ende des Textes gescrollt hast (sobald das Textende im Fenster zu sehen ist). Gelesene Artikel starten immer oben. Ohne Verbindung wird die Stelle vorgemerkt und später übertragen.

## Zurück zur Liste

Im Lesemodus steht „← Zur Liste“ oben als normaler Link. Scrollst du weiter, erscheint er zusätzlich als schwebende Bubble oben links über dem Text, sodass du jederzeit zurückkommst. Am Ende des Artikels steht unten noch einmal ein normaler Link; sobald der zu sehen ist, verschwindet die Bubble.

## Bilder

Beim Speichern lädt der Server die Bilder eines Artikels im Hintergrund herunter, verkleinert sie auf höchstens 1600 Pixel und zeigt im Lesemodus diese lokalen Kopien. Dadurch funktionieren auch Bilder von Servern, die das Einbinden auf fremden Seiten verhindern, und sie bleiben erhalten, wenn das Original verschwindet. Direkt nach dem Speichern stehen noch die Originale im Text, nach wenigen Sekunden beim nächsten Öffnen die lokalen Bilder. Ältere Artikel und Importe bekommen ihre Bilder über „Bilder nachladen“ im Lesemodus. Von den Originalen wird nichts aufbewahrt. Einstellbar sind `IMAGE_STORE_ENABLED` (Standard an), `IMAGE_CONCURRENCY` und `IMAGE_MAX_BYTES`. Die Bilder liegen in der Datenbank und vergrößern sie um etwa 0,2–0,5 MB je Artikel. Mit „Offline-Lesen“ legt das Gerät die Bilder der gespeicherten Artikel zusätzlich in einem eigenen Speicher des Browsers ab, sie sind also auch ohne Internet da und verschwinden beim Abmelden.

## Installieren und offline lesen (PWA)

Die Leseliste lässt sich wie eine App installieren („Zum Startbildschirm hinzufügen“ bzw. „App installieren“) und gespeicherte Artikel auch ohne Internet lesen, zum Beispiel im Zug.

**Wichtig: Das funktioniert nur über HTTPS.** Browser erlauben den dafür nötigen Service Worker nur auf `localhost` und auf HTTPS-Adressen. Über reines HTTP (z. B. im Heimnetz ohne Proxy) läuft die App normal, aber ohne Installation und ohne Start ohne Netz. Dein Caddy mit einer Domain und Zertifikat erfüllt das. Hinter Tailscale geht es mit `tailscale serve` und dem HTTPS-Namen des Geräts. Lokal funktioniert `http://localhost:3000`.

- Das Gerät speichert die 30 neuesten ungelesenen Artikel samt Markierungen und Notizen und jeden Artikel, den du geöffnet hast (14 Tage, höchstens 30). Der Abgleich läuft beim Öffnen der Liste im Hintergrund, unter „Offline-Lesen“ im Konto-Menü lässt er sich auch von Hand starten, ausschalten oder löschen.
- Ohne Verbindung (oder bei sehr schlechter) siehst du die gespeicherten Artikel, kannst sie lesen, durchsuchen und als gelesen markieren. Das Markieren und die Leseposition werden nachgeholt, sobald wieder Verbindung besteht. Neue Artikel speichern, Schlagwörter, Markierungen und Notizen ändern braucht eine Verbindung.
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
cd backend
DATABASE_PATH=./data/app.db RATE_LIMIT_ENABLED=false COOKIE_SECURE=false cargo run   # http://localhost:3000
```

Ein Prozess liefert Frontend und Schnittstelle. Im Debug-Build wird `frontend/public` von der Platte gelesen, Änderungen sieht man nach dem Neuladen. Das Image lokal bauen: `docker build -t leseliste .` im Repository-Wurzelverzeichnis.
