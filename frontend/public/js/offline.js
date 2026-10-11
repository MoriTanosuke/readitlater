// Artikel für das Lesen ohne Internet auf diesem Gerät (IndexedDB).
//
// Gespeichert werden die neuesten ungelesenen Artikel und jeder Artikel, den der Benutzer
// geöffnet hat (14 Tage, höchstens 30). Es sind dieselben bereinigten Daten, die die
// Schnittstelle liefert. Alles wird beim Abmelden, bei abgelaufener Sitzung und beim
// Kontolöschen entfernt und beim Anmelden eines anderen Benutzers verworfen.
//
// Offline geändert werden kann nur der Gelesen-Status; er wird vorgemerkt und beim nächsten
// Abgleich gesendet. Alle anderen Änderungen brauchen eine Verbindung.
//
// Der Speicher ist eine Bequemlichkeit: Ist IndexedDB nicht verfügbar (z. B. im privaten
// Modus), arbeitet die App wie bisher und alle Funktionen hier liefern leere Ergebnisse.

import { ApiError } from './api.js';

const DB_NAME = 'leseliste-offline';
const DB_VERSION = 1;
const ENABLED_KEY = 'leseliste.offline.enabled';

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

export function clearAll() {
  return safely(
    () =>
      transact(['meta', 'articles', 'queue'], 'readwrite', (meta, articles, queue) => {
        meta.clear();
        articles.clear();
        queue.clear();
      }),
    null,
  );
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

export function remove(id) {
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
    created_at: article.created_at,
    tags: article.tags ?? [],
    highlights: article.highlights ?? [],
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
  return items.map(({ content, lang, highlights, opened_at, ...summary }) => summary);
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
// Gelesen-Status ohne Verbindung
// ---------------------------------------------------------------------------

/** Merkt eine Änderung des Gelesen-Status vor und übernimmt sie in die gespeicherte Kopie. */
export async function queueRead(id, isRead) {
  await safely(
    () =>
      transact(['articles', 'queue'], 'readwrite', (articles, queue) => {
        queue.put({ id, is_read: isRead });
        const request = articles.get(id);
        request.onsuccess = () => {
          if (request.result) articles.put({ ...request.result, is_read: isRead });
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
  const result = { stored: 0, removed: 0, flushed: 0, offline: false, skipped: false };
  try {
    for (const change of await pendingReads()) {
      try {
        await api.updateArticle(change.id, { is_read: change.is_read });
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
      if (stored) {
        // Status, Titel und Schlagwörter können sich geändert haben; der Inhalt nicht.
        await putArticle({ ...stored, ...summary, content: stored.content, highlights: stored.highlights });
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
