// Bilder offline: Der Offline-Abgleich legt die Bilder gespeicherter Artikel im Cache-Speicher
// ab (Cache API, Name „leseliste-images“), hält ihn im Takt mit den Artikeln und leert ihn mit
// allem anderen. jsdom hat keine Cache API; ein einfacher Nachbau dient als Browser.
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { MIT_BILDERN, makePng } from './fixtures.mjs';
import { setup, sleep, steps } from './helpers.mjs';

// ---------------------------------------------------------------------------
// Nachbau der Cache API
// ---------------------------------------------------------------------------

const stores = new Map(); // Name -> Map(Pfad -> { status, headers, bytes })
const pathOf = (request) => new URL(typeof request === 'string' ? request : request.url, 'http://app.test').pathname;

globalThis.caches = {
  async open(name) {
    if (!stores.has(name)) stores.set(name, new Map());
    const entries = stores.get(name);
    return {
      async match(request) {
        const entry = entries.get(pathOf(request));
        return entry ? new Response(entry.bytes, { status: entry.status, headers: entry.headers }) : undefined;
      },
      async put(request, response) {
        entries.set(pathOf(request), {
          status: response.status,
          headers: [...response.headers],
          bytes: Buffer.from(await response.arrayBuffer()),
        });
      },
      async delete(request) {
        return entries.delete(pathOf(request));
      },
      async keys() {
        return [...entries.keys()].map((pathname) => ({ url: `http://app.test${pathname}` }));
      },
    };
  },
  async delete(name) {
    return stores.delete(name);
  },
};
const cached = () => [...(stores.get('leseliste-images')?.keys() ?? [])].sort();

// ---------------------------------------------------------------------------
// Anwendung
// ---------------------------------------------------------------------------

const pages = {
  '/artikel.html': MIT_BILDERN,
  '/bild/da.png': { type: 'image/png', body: makePng(2000, 1200) },
  // '/bild/fehlt.png' gibt es zunächst nicht.
};
const { $, visible, until, site, setOffline, teardown } = await setup(pages);
after(teardown);

const json = async (path) => (await fetch(path)).json();
const send = (path, method, body = {}) =>
  fetch(path, {
    method,
    headers: { 'X-Requested-With': 'fetch', 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });

async function untilAsync(check, label, timeout = 15_000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    if (await check()) return;
    await sleep(60);
  }
  throw new Error(`Zeitüberschreitung: ${label}`);
}

/** „Jetzt aktualisieren“ auf der Seite Offline-Lesen. */
async function syncNow() {
  window.location.hash = '#/account/offline';
  await until(() => visible('view-offline'), 'Seite Offline-Lesen');
  const api = await import('../public/js/api.js');
  const offline = await import('../public/js/offline.js');
  return offline.sync(api.api);
}

test('Bilder gespeicherter Artikel liegen im Cache und folgen den Artikeln', { timeout: 120_000 }, async (t) => {
  const ok = steps(t);
  const offline = await import('../public/js/offline.js');

  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  $('form-auth').elements.email.value = 'zug@example.org';
  $('form-auth').elements.password.value = 'passwort-12345';
  $('form-auth').requestSubmit();
  await until(() => visible('view-home'), 'Startseite');
  $('add-url').value = site.url('/artikel.html');
  $('form-add').requestSubmit();
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'Artikel gespeichert');
  const id = (await json('/api/articles'))[0].id;

  // ---- Das erste Bild wird auf dem Server fertig (das zweite gibt es nicht)
  await untilAsync(async () => Object.keys((await json(`/api/articles/${id}`)).images).length === 1, 'erstes Bild fertig');
  const first = Object.values((await json(`/api/articles/${id}`)).images)[0];
  const original = Buffer.from(await (await fetch(first.src)).arrayBuffer());

  // ---- Abgleich: Bild landet im Cache, byte-genau
  const result = await syncNow();
  assert.equal(result.images, 1);
  assert.deepEqual(cached(), [first.src]);
  const entry = await (await caches.open('leseliste-images')).match(first.src);
  assert.equal(entry.headers.get('content-type'), 'image/png');
  assert.ok(Buffer.from(await entry.arrayBuffer()).equals(original), 'gleiche Bytes wie vom Server');
  assert.equal(Object.keys((await offline.getArticle(id)).images).length, 1);
  ok('Abgleich legt das fertige Bild im Cache ab');

  // ---- Ein weiterer Abgleich holt nichts doppelt
  assert.equal((await syncNow()).images, 0);

  // ---- Ohne Verbindung: nichts geht kaputt, was im Cache liegt, bleibt
  setOffline(true);
  const stored = await offline.getArticle(id);
  const unknown = { ...stored, images: { ...stored.images, 'https://x.example/neu.png': { src: '/api/articles/1/images/999', width: 1, height: 1 } } };
  assert.equal(await offline.cacheImages(unknown), 0, 'ohne Verbindung still beendet');
  assert.deepEqual(cached(), [first.src]);
  setOffline(false);
  ok('ohne Verbindung endet das Vorladen still, der Cache bleibt');

  // ---- Zweites Bild wird später fertig: der Abgleich erkennt es an image_count
  pages['/bild/fehlt.png'] = { type: 'image/png', body: makePng(300, 200) };
  assert.equal((await send(`/api/articles/${id}/refresh-images`, 'POST')).status, 200);
  await untilAsync(async () => Object.keys((await json(`/api/articles/${id}`)).images).length === 2, 'zweites Bild fertig');
  const second = Object.values((await json(`/api/articles/${id}`)).images).find((image) => image.src !== first.src);
  assert.equal((await syncNow()).images, 1);
  assert.deepEqual(cached(), [first.src, second.src].sort());
  assert.equal(Object.keys((await offline.getArticle(id)).images).length, 2, 'Kopie aufgefrischt');
  ok('später fertig gewordene Bilder werden nachgeholt');

  // ---- Fremde Einträge, die zu keinem Artikel gehören, werden aufgeräumt
  const cache = await caches.open('leseliste-images');
  await cache.put('/api/articles/9999/images/1', new Response('x', { headers: { 'content-type': 'image/png' } }));
  assert.equal(cached().length, 3);
  await syncNow();
  assert.deepEqual(cached(), [first.src, second.src].sort());
  ok('verwaiste Einträge werden entfernt');

  // ---- Eine Kopie zu entfernen (etwa nach dem Löschen im Lesemodus) nimmt ihre Bilder sofort mit
  const keep = await offline.getArticle(id);
  await offline.remove(id);
  assert.deepEqual(cached(), []);
  await offline.putArticle(keep);
  ok('remove() entfernt die Bilder des Artikels');

  // ---- Artikel gelöscht: Abgleich entfernt Kopie und Bilder
  assert.equal((await send(`/api/articles/${id}`, 'DELETE')).status, 204);
  await syncNow();
  assert.equal(await offline.getArticle(id), null);
  assert.deepEqual(cached(), []);
  ok('gelöschte Artikel nehmen ihre Bilder mit');

  // ---- Abmelden leert den Bilder-Cache mit allem anderen
  $('add-url').value = site.url('/artikel.html');
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  $('form-add').requestSubmit();
  await untilAsync(async () => (await json('/api/articles')).length === 1, 'Artikel erneut gespeichert');
  const again = (await json('/api/articles'))[0].id;
  await untilAsync(async () => Object.keys((await json(`/api/articles/${again}`)).images).length === 2, 'Bilder fertig');
  await syncNow();
  assert.equal(cached().length, 2);
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  $('btn-logout').click();
  await until(() => visible('view-auth'), 'abgemeldet');
  await untilAsync(() => !stores.has('leseliste-images'), 'Cache geleert');
  ok('Abmelden löscht den Bilder-Cache');
});
