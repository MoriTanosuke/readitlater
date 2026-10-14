// Artikel für das Lesen ohne Internet auf diesem Gerät (IndexedDB).
//
// Gespeichert werden die neuesten ungelesenen Artikel und jeder Artikel, den der Benutzer
// geöffnet hat (14 Tage, höchstens 30). Es sind dieselben bereinigten Daten, die die
// Schnittstelle liefert. Alles wird beim Abmelden, bei abgelaufener Sitzung und beim
// Kontolöschen entfernt und beim Anmelden eines anderen Benutzers verworfen.
//
// Offline geändert werden kann nur der Gelesen-Status und die Leseposition; sie werden
// vorgemerkt und beim nächsten Abgleich gesendet. Alle anderen Änderungen brauchen eine
// Verbindung.
//
// Die verkleinerten Bilder der gespeicherten Artikel liegen im Cache-Speicher des Browsers
// (Cache API, Name `leseliste-images`), nicht im normalen HTTP-Cache, den der Browser nach
// Belieben räumt. Geschrieben wird nur hier; der Service Worker (sw.js) liest sie für genau die
// Adressen `/api/articles/{id}/images/{id}` und holt nie selbst etwas. Sie werden mit allem
// anderen gelöscht (Abmelden, abgelaufene Sitzung, Kontolöschen, anderer Benutzer).
//
// Der Speicher ist eine Bequemlichkeit: Ist IndexedDB nicht verfügbar (z. B. im privaten
// Modus), arbeitet die App wie bisher und alle Funktionen hier liefern leere Ergebnisse.

import { ApiError } from './api.js';

const DB_NAME = 'leseliste-offline';
const DB_VERSION = 1;
const ENABLED_KEY = 'leseliste.offline.enabled';
/** Cache der Bilder; sw.js liest ihn unter demselben Namen. */
export const IMAGE_CACHE = 'leseliste-images';
const IMAGE_PATH = /^\/api\/articles\/\d+\/images\/\d+$/;
const IMAGE_TIMEOUT_MS = 30_000;

/** Wie viele der neuesten ungelesenen Artikel automatisch bereitgehalten werden. */
export const UNREAD_LIMIT = 30;
const KEEP_OPENED_MS = 14 * 24 * 60 * 60 * 1000;
const MAX_OPENED = 30;

// ---------------------------------------------------------------------------
// Datenbank
// ---------------------------------------------------------------------------

let dbPromise = null;

function openDb() {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise((resolve, reject) => {
    if (typeof indexedDB === 'undefined') {
      reject(new Error('IndexedDB nicht verfügbar'));
      return;
    }
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      db.createObjectStore('meta');
      db.createObjectStore('articles', { keyPath: 'id' });
      db.createObjectStore('queue', { keyPath: 'id' });
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  dbPromise.catch(() => {
    dbPromise = null;
  });
  return dbPromise;
}

/** Führt `work(stores...)` in einer Transaktion aus und liefert das Ergebnis eines Requests. */
async function transact(names, mode, work) {
  const db = await openDb();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(names, mode);
    let result;
    tx.oncomplete = () => resolve(result);
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error ?? new Error('Transaktion abgebrochen'));
    const out = work(...names.map((name) => tx.objectStore(name)));
    if (out && typeof out === 'object' && 'readyState' in out) {
      out.onsuccess = () => {
        result = out.result;
      };
    }
  });
}

/** Fehler des Speichers dürfen die App nie stören. */
async function safely(task, fallback) {
  try {
    return await task();
  } catch {
    return fallback;
  }
}

// ---------------------------------------------------------------------------
// Einstellung
// ---------------------------------------------------------------------------

export function isEnabled() {
  try {
    return window.localStorage.getItem(ENABLED_KEY) !== '0';
  } catch {
    return true;
  }
}

export async function setEnabled(enabled) {
  try {
    window.localStorage.setItem(ENABLED_KEY, enabled ? '1' : '0');
  } catch {
    // ohne localStorage bleibt es bei der Voreinstellung
  }
  if (!enabled) await clearAll();
  else navigator.storage?.persist?.().catch(() => {}); // Speicher nicht ungefragt räumen lassen
}

// ---------------------------------------------------------------------------
// Besitzer, Löschen
// ---------------------------------------------------------------------------

export function getOwner() {
  return safely(() => transact(['meta'], 'readonly', (meta) => meta.get('owner')), null).then(
    (owner) => owner ?? null,
  );
}

/** Merkt sich den angemeldeten Benutzer; Daten eines anderen Benutzers werden verworfen. */
export async function adopt(user) {
  const owner = await getOwner();
  if (owner && owner.id !== user.id) await clearAll();
  await safely(
    () => transact(['meta'], 'readwrite', (meta) => meta.put({ id: user.id, email: user.email }, 'owner')),
    null,
  );
}

export async function clearAll() {
  await safely(
    () =>
      transact(['meta', 'articles', 'queue'], 'readwrite', (meta, articles, queue) => {
        meta.clear();
        articles.clear();
        queue.clear();
      }),
    null,
  );
  await safely(async () => {
    if (typeof caches !== 'undefined') await caches.delete(IMAGE_CACHE);
  }, null);
}

// ---------------------------------------------------------------------------
// Bilder
// ---------------------------------------------------------------------------

async function openImageCache() {
  return safely(async () => (typeof caches === 'undefined' ? null : await caches.open(IMAGE_CACHE)), null);
}

/** Adressen der lokal abgelegten Bilder eines Artikels (nur die erwartete Form). */
function imageSrcs(article) {
  return Object.values(article?.images ?? {})
    .map((image) => image?.src)
    .filter((src) => typeof src === 'string' && IMAGE_PATH.test(src));
}

/**
 * Legt die Bilder eines Artikels im Cache ab, soweit sie dort noch fehlen. Ohne Verbindung,
 * bei vollem Speicher oder ohne Cache-Unterstützung endet es still. Gibt die Zahl neu
 * abgelegter Bilder zurück.
 */
export async function cacheImages(article) {
  if (!isEnabled()) return 0;
  const cache = await openImageCache();
  if (!cache) return 0;
  let added = 0;
  for (const src of imageSrcs(article)) {
    try {
      if (await cache.match(src)) continue;
      const response = await fetch(src, {
        credentials: 'same-origin',
        signal: AbortSignal.timeout?.(IMAGE_TIMEOUT_MS),
      });
      const type = response.headers.get('content-type') ?? '';
      if (!response.ok || !type.startsWith('image/')) continue; // dieses Bild gibt es (noch) nicht
      await cache.put(src, response);
      added += 1;
    } catch {
      return added; // keine Verbindung oder Speicher voll: beim nächsten Abgleich weiter
    }
  }
  return added;
}

/** Entfernt die Bilder eines Artikels aus dem Cache. */
async function dropImages(article) {
  const cache = await openImageCache();
  if (!cache) return;
  for (const src of imageSrcs(article)) await safely(() => cache.delete(src), null);
}

/**
 * Hält den Bilder-Cache im Takt mit den gespeicherten Artikeln: Fehlendes wird geholt, alles,
 * was zu keinem gespeicherten Artikel mehr gehört, wird entfernt.
 */
async function syncImages() {
  const articles = await allArticles();
  const cache = await openImageCache();
  if (!cache) return 0;
  const wanted = new Set(articles.flatMap(imageSrcs));
  await safely(async () => {
    for (const request of await cache.keys()) {
      const key = new URL(request.url).pathname;
      if (!wanted.has(key)) await cache.delete(request);
    }
  }, null);
  let added = 0;
  for (const article of articles) added += await cacheImages(article);
  return added;
}

// ---------------------------------------------------------------------------
// Artikel
// ---------------------------------------------------------------------------

export function getArticle(id) {
  return safely(() => transact(['articles'], 'readonly', (a) => a.get(id)), null).then((a) => a ?? null);
}

function allArticles() {
  return safely(() => transact(['articles'], 'readonly', (a) => a.getAll()), []).then((list) => list ?? []);
}

export async function remove(id) {
  const article = await getArticle(id);
  if (article) await dropImages(article);
  return safely(
    () =>
      transact(['articles', 'queue'], 'readwrite', (articles, queue) => {
        articles.delete(id);
        queue.delete(id);
      }),
    null,
  );
}

/** Speichert einen Artikel mit Inhalt. `opened`: der Benutzer hat ihn selbst geöffnet. */
export async function putArticle(article, { opened = false } = {}) {
  if (!isEnabled()) return;
  const previous = await getArticle(article.id);
  const record = {
    id: article.id,
    url: article.url,
    title: article.title,
    excerpt: article.excerpt,
    content: article.content,
    lang: article.lang ?? null,
    is_read: article.is_read,
    read_pos: article.read_pos ?? 0,
    created_at: article.created_at,
    tags: article.tags ?? [],
    highlights: article.highlights ?? [],
    images: article.images ?? {},
    opened_at: opened ? Date.now() : (previous?.opened_at ?? null),
  };
  await safely(() => transact(['articles'], 'readwrite', (a) => a.put(record)), null);
}

export function count() {
  return allArticles().then((list) => list.length);
}

export async function stats() {
  const list = await allArticles();
  const bytes = list.reduce((sum, a) => sum + (a.content?.length ?? 0) + (a.title?.length ?? 0), 0);
  return { count: list.length, bytes };
}

/** Beim Vergleich zählen Groß-/Kleinschreibung und Akzente nicht (wie die Suche im Backend). */
function fold(text) {
  return text.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();
}

/** Liste für die Ansicht ohne Verbindung: Filter wie online, neueste zuerst, ohne Inhalt. */
export async function list(filter = {}) {
  const words = fold(filter.q ?? '').split(/\s+/).filter(Boolean);
  const items = (await allArticles())
    .filter((a) => filter.read === '' || filter.read === undefined || a.is_read === (filter.read === 'true'))
    .filter((a) => !filter.tag || a.tags.some((t) => t.toLowerCase() === filter.tag.toLowerCase()))
    .filter((a) => {
      if (words.length === 0) return true;
      const haystack = fold(`${a.title} ${a.excerpt} ${a.tags.join(' ')} ${a.url}`);
      return words.every((w) => haystack.includes(w));
    })
    .sort((a, b) => (a.created_at < b.created_at ? 1 : a.created_at > b.created_at ? -1 : b.id - a.id));
  return items.map(({ content, lang, highlights, images, opened_at, read_pos, ...summary }) => summary);
}

/** Schlagwörter mit Anzahl über die gespeicherten Artikel. */
export async function tagCounts() {
  const counts = new Map();
  for (const article of await allArticles()) {
    for (const tag of article.tags) counts.set(tag, (counts.get(tag) ?? 0) + 1);
  }
  return [...counts].map(([name, n]) => ({ name, count: n })).sort((a, b) => a.name.localeCompare(b.name));
}

// ---------------------------------------------------------------------------
// Gelesen-Status und Leseposition ohne Verbindung
// ---------------------------------------------------------------------------

/**
 * Merkt Änderungen vor (`is_read` und/oder `read_pos`), die sich ohne Verbindung nicht senden
 * ließen, und übernimmt sie in die gespeicherte Kopie. Je Artikel gilt die letzte Änderung.
 * Ein gelesener Artikel hat die Position 0 (wie im Backend).
 */
export async function queueChange(id, changes) {
  const applied = { ...changes };
  if (applied.is_read === true) applied.read_pos = 0;
  await safely(
    () =>
      transact(['articles', 'queue'], 'readwrite', (articles, queue) => {
        const pending = queue.get(id);
        pending.onsuccess = () => {
          const entry = { ...(pending.result ?? {}), ...changes, id };
          if (entry.is_read === true) entry.read_pos = 0;
          queue.put(entry);
        };
        const request = articles.get(id);
        request.onsuccess = () => {
          if (request.result) articles.put({ ...request.result, ...applied });
        };
      }),
    null,
  );
}

/** Merkt eine Änderung des Gelesen-Status vor und übernimmt sie in die gespeicherte Kopie. */
export function queueRead(id, isRead) {
  return queueChange(id, { is_read: isRead });
}

/** Übernimmt die Leseposition in die gespeicherte Kopie (falls es eine gibt). */
export function setPosition(id, pos) {
  return safely(
    () =>
      transact(['articles'], 'readwrite', (articles) => {
        const request = articles.get(id);
        request.onsuccess = () => {
          if (request.result) articles.put({ ...request.result, read_pos: pos });
        };
      }),
    null,
  );
}

/**
 * Eine direkt gesendete Position ersetzt die vorgemerkte; sonst würde der Abgleich später die
 * ältere nachsenden und die neuere überschreiben.
 */
export function dropQueuedPosition(id) {
  return safely(
    () =>
      transact(['queue'], 'readwrite', (queue) => {
        const pending = queue.get(id);
        pending.onsuccess = () => {
          const entry = pending.result;
          if (!entry || entry.read_pos === undefined) return;
          const { read_pos, ...rest } = entry;
          if (rest.is_read === undefined) queue.delete(id);
          else queue.put(rest);
        };
      }),
    null,
  );
}

function pendingReads() {
  return safely(() => transact(['queue'], 'readonly', (q) => q.getAll()), []).then((l) => l ?? []);
}

function dropQueued(id) {
  return safely(() => transact(['queue'], 'readwrite', (q) => q.delete(id)), null);
}

// ---------------------------------------------------------------------------
// Abgleich mit dem Server
// ---------------------------------------------------------------------------

let running = null;

/**
 * Sendet vorgemerkte Änderungen, lädt fehlende ungelesene Artikel nach und räumt auf.
 * Ohne Verbindung endet der Abgleich still (`offline: true`); eine abgelaufene Sitzung
 * (ApiError 401) wird weitergereicht.
 */
export function sync(api) {
  if (!isEnabled()) return Promise.resolve({ stored: 0, removed: 0, flushed: 0, offline: false, skipped: true });
  if (!running) {
    running = runSync(api).finally(() => {
      running = null;
    });
  }
  return running;
}

async function runSync(api) {
  const result = { stored: 0, removed: 0, flushed: 0, images: 0, offline: false, skipped: false };
  try {
    for (const change of await pendingReads()) {
      try {
        const body = {};
        if (change.is_read !== undefined) body.is_read = change.is_read;
        if (change.read_pos !== undefined) body.read_pos = change.read_pos;
        await api.updateArticle(change.id, body);
        result.flushed += 1;
      } catch (error) {
        if (!(error instanceof ApiError && error.status === 404)) throw error;
        await remove(change.id); // Artikel gibt es nicht mehr
        continue;
      }
      await dropQueued(change.id);
    }

    const unread = await api.listArticles(UNREAD_LIMIT, 0, { read: 'false' });
    const wanted = new Set(unread.map((a) => a.id));
    for (const summary of unread) {
      const stored = await getArticle(summary.id);
      if (stored && (summary.image_count ?? 0) !== Object.keys(stored.images ?? {}).length) {
        // Inzwischen sind Bilder fertig geworden (oder weggefallen): den Artikel neu holen.
        try {
          await putArticle(await api.getArticle(summary.id));
        } catch (error) {
          if (!(error instanceof ApiError && error.status === 404)) throw error;
        }
        continue;
      }
      if (stored) {
        // Status, Titel und Schlagwörter können sich geändert haben; der Inhalt nicht.
        await putArticle({ ...stored, ...summary, content: stored.content, highlights: stored.highlights, images: stored.images });
        continue;
      }
      try {
        await putArticle(await api.getArticle(summary.id));
        result.stored += 1;
      } catch (error) {
        if (!(error instanceof ApiError && error.status === 404)) throw error;
      }
    }
    result.removed = await prune(wanted);
    result.images = await syncImages();
  } catch (error) {
    if (error instanceof ApiError && error.status === 0) result.offline = true;
    else throw error;
  }
  return result;
}

/** Entfernt, was weder zu den neuesten ungelesenen gehört noch kürzlich geöffnet wurde. */
async function prune(wanted) {
  const now = Date.now();
  const all = await allArticles();
  const drop = new Set();
  const opened = [];
  for (const article of all) {
    if (wanted.has(article.id)) continue;
    if (article.opened_at && now - article.opened_at < KEEP_OPENED_MS) opened.push(article);
    else drop.add(article.id);
  }
  opened.sort((a, b) => b.opened_at - a.opened_at);
  for (const article of opened.slice(MAX_OPENED)) drop.add(article.id);
  for (const id of drop) await remove(id);
  return drop.size;
}
