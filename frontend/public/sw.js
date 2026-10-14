// Service Worker: hält nur die App-Dateien (HTML, CSS, JavaScript, Icons) für den Start ohne
// Internet bereit. Er fasst /api/ nie an: Artikel und Antworten der Schnittstelle landen nicht
// im Cache dieses Workers (die gespeicherten Artikel verwaltet js/offline.js, nur für den
// angemeldeten Benutzer und mit Löschen beim Abmelden).
//
// Eine einzige Ausnahme gilt für die lokal abgelegten Artikelbilder (`/api/articles/{id}/images/{id}`):
// Sie liegen im Cache `leseliste-images`, den allein js/offline.js befüllt und beim Abmelden
// leert. Der Worker liest ihn nur (zuerst der Cache, sonst das Netz) und speichert nie selbst
// etwas. So sind die Bilder gespeicherter Artikel auch ohne Internet da.
//
// Strategie „Netz zuerst mit Zeitlimit“: Antwortet der Server innerhalb von vier Sekunden, gilt
// die frische Datei (und wird im Cache aufgefrischt). Sonst, etwa im Zug bei schlechtem Empfang
// oder ohne Netz, kommt die zuletzt gespeicherte Datei. So passen HTML und Skripte zusammen,
// ohne dass bei Änderungen eine Versionsnummer gepflegt werden muss.

const CACHE = 'leseliste-shell-v1';
const IMAGE_CACHE = 'leseliste-images'; // gleicher Name wie in js/offline.js
const IMAGE_PATH = /^\/api\/articles\/\d+\/images\/\d+$/;
const TIMEOUT_MS = 4000;

// Wird beim Installieren vollständig geladen. Alles, was die Seite später nachlädt, kommt
// zusätzlich beim ersten Abruf in den Cache; ein Test prüft, dass diese Liste vollständig ist.
const SHELL = [
  '/',
  '/style.css',
  '/manifest.webmanifest',
  '/js/api.js',
  '/js/app.js',
  '/js/backlink.js',
  '/js/highlights.js',
  '/js/offline.js',
  '/js/position.js',
  '/js/reading.js',
  '/js/safe-html.js',
  '/icons/icon-192.png',
  '/icons/icon-512.png',
];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(keys.filter((key) => key !== CACHE && key !== IMAGE_CACHE).map((key) => caches.delete(key))),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (event) => {
  const request = event.request;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;
  if (IMAGE_PATH.test(url.pathname)) {
    event.respondWith(cachedImage(request, url));
    return;
  }
  if (url.pathname.startsWith('/api/')) return; // nie die Schnittstelle zwischenspeichern
  event.respondWith(networkFirst(event, request, url));
});

/** Artikelbild: der gespeicherte Stand, sonst das Netz. Schreibt nie in den Cache. */
async function cachedImage(request, url) {
  try {
    const cache = await caches.open(IMAGE_CACHE);
    const hit = await cache.match(url.pathname);
    if (hit) return hit;
  } catch {
    // Cache nicht lesbar: wie ohne Worker weiter zum Netz
  }
  return fetch(request);
}

/** Alle Seitenaufrufe teilen sich einen Eintrag („/“), auch mit Suchparametern (Teilen). */
function cacheKey(request, url) {
  return request.mode === 'navigate' ? '/' : url.pathname;
}

async function networkFirst(event, request, url) {
  const cache = await caches.open(CACHE);
  const key = cacheKey(request, url);
  const network = fetch(request).then((response) => {
    if (response.ok && response.type === 'basic') cache.put(key, response.clone());
    return response;
  });
  network.catch(() => {}); // verspätete Fehler sind kein Problem
  event.waitUntil(network.catch(() => {}));

  const timeout = new Promise((_, reject) => setTimeout(() => reject(new Error('Zeitlimit')), TIMEOUT_MS));
  try {
    return await Promise.race([network, timeout]);
  } catch {
    const cached = await cache.match(key);
    if (cached) return cached;
    return network; // nichts gespeichert: auf das Netz warten (oder dessen Fehler weitergeben)
  }
}
