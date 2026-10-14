// PWA: Manifest, Icons und Service Worker (ohne Browser, der Worker läuft in einer vm).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const PUBLIC = path.join(path.dirname(fileURLToPath(import.meta.url)), '../public');
const read = (file) => readFileSync(path.join(PUBLIC, file), 'utf8');

test('Manifest ist installierbar und verweist auf vorhandene Icons', () => {
  const manifest = JSON.parse(read('manifest.webmanifest'));
  assert.equal(manifest.display, 'standalone');
  assert.equal(manifest.start_url, '/');
  assert.equal(manifest.scope, '/');
  assert.ok(manifest.name && manifest.short_name);
  assert.equal(manifest.lang, 'de');

  const sizes = manifest.icons.map((icon) => icon.sizes);
  assert.ok(sizes.includes('192x192') && sizes.includes('512x512'), 'Icons 192 und 512');
  assert.ok(manifest.icons.some((icon) => icon.purpose === 'maskable'), 'maskierbares Icon');
  for (const icon of manifest.icons) {
    const file = icon.src.replace(/^\//, '');
    assert.ok(existsSync(path.join(PUBLIC, file)), `${icon.src} fehlt`);
    // PNG-Kopf lesen: Größe muss zur Angabe passen
    const png = readFileSync(path.join(PUBLIC, file));
    assert.equal(png.subarray(1, 4).toString(), 'PNG');
    const [w, h] = [png.readUInt32BE(16), png.readUInt32BE(20)];
    assert.equal(`${w}x${h}`, icon.sizes, icon.src);
  }

  // Teilen aus anderen Apps: GET auf die Startseite, Parameter werden von der App ausgewertet
  assert.equal(manifest.share_target.method, 'GET');
  assert.equal(manifest.share_target.action, '/');
  assert.deepEqual(Object.keys(manifest.share_target.params).sort(), ['text', 'title', 'url']);
});

test('index.html bindet Manifest, Icons und Farbe ein', () => {
  const html = read('index.html');
  assert.match(html, /<link rel="manifest" href="\/manifest\.webmanifest">/);
  assert.match(html, /<meta name="theme-color"/);
  assert.match(html, /rel="apple-touch-icon" href="\/icons\/apple-touch-icon\.png"/);
  assert.ok(existsSync(path.join(PUBLIC, 'icons/apple-touch-icon.png')));
});

/** Lädt sw.js in eine vm mit nachgebauten Browser-Schnittstellen. */
function loadWorker({ fetchImpl, cached = new Map(), cacheKeys = [] } = {}) {
  const handlers = {};
  const puts = [];
  const deleted = [];
  const cache = {
    async addAll() {},
    async put(key, response) { puts.push(key); cached.set(key, response); },
    async match(key) { return cached.get(key); },
  };
  const sandbox = {
    self: {
      location: { origin: 'https://leseliste.example' },
      addEventListener: (type, fn) => { handlers[type] = fn; },
      skipWaiting: async () => {},
      clients: { claim: async () => {} },
    },
    caches: {
      open: async () => cache,
      keys: async () => cacheKeys,
      delete: async (name) => { deleted.push(name); return true; },
    },
    fetch: fetchImpl ?? (async () => ({ ok: true, type: 'basic', clone() { return this; } })),
    URL,
    Promise,
    // Das Zeitlimit von vier Sekunden läuft im Test sofort ab.
    setTimeout: (fn) => queueMicrotask(fn),
  };
  sandbox.self.caches = sandbox.caches;
  vm.runInNewContext(read('sw.js'), sandbox);
  const run = async (request) => {
    let responded;
    handlers.fetch({
      request,
      respondWith: (promise) => { responded = promise; },
      waitUntil: () => {},
    });
    const response = responded ? await responded : undefined;
    return { intercepted: responded !== undefined, response };
  };
  const activate = async () => {
    let waited;
    handlers.activate({ waitUntil: (promise) => { waited = promise; } });
    await waited;
  };
  return { run, puts, cached, deleted, activate };
}

const get = (pathname, extra = {}) => ({
  method: 'GET', url: `https://leseliste.example${pathname}`, mode: 'no-cors', ...extra,
});

test('Service Worker fasst die Schnittstelle, fremde Adressen und Schreibzugriffe nie an', async () => {
  const worker = loadWorker();
  for (const request of [
    get('/api/articles'),
    get('/api/articles/5'),
    get('/api/export'),
    get('/api/me'),
    { method: 'POST', url: 'https://leseliste.example/', mode: 'cors' },
    { method: 'DELETE', url: 'https://leseliste.example/api/articles/1', mode: 'cors' },
    { method: 'GET', url: 'https://fremd.example/bild.png', mode: 'no-cors' },
  ]) {
    assert.equal((await worker.run(request)).intercepted, false, `${request.method} ${request.url}`);
  }
  assert.deepEqual(worker.puts, [], 'nichts davon landet im Cache');
});

test('Service Worker liefert gespeicherte Artikelbilder zuerst aus dem Cache und schreibt nie hinein', async () => {
  const image = '/api/articles/7/images/3';
  const stored = { tag: 'gespeichert' };
  let worker = loadWorker({
    fetchImpl: async () => { throw new TypeError('offline'); },
    cached: new Map([[image, stored]]),
  });
  let result = await worker.run(get(image, { mode: 'no-cors' }));
  assert.equal(result.intercepted, true);
  assert.equal(result.response, stored, 'ohne Netz kommt das gespeicherte Bild');

  // Mit Netz gewinnt trotzdem der Cache (Bilder ändern sich nie), das Netz wird nicht gefragt.
  let calls = 0;
  worker = loadWorker({
    fetchImpl: async () => { calls += 1; return { ok: true, type: 'basic', clone() { return this; } }; },
    cached: new Map([[image, stored]]),
  });
  assert.equal((await worker.run(get(image))).response, stored);
  assert.equal(calls, 0);

  // Nicht gespeichert: Netz, und der Worker legt nichts ab (das tut allein offline.js).
  const fresh = { ok: true, type: 'basic', tag: 'frisch', clone() { return this; } };
  worker = loadWorker({ fetchImpl: async () => fresh });
  result = await worker.run(get(image));
  assert.equal(result.response, fresh);
  assert.deepEqual(worker.puts, []);

  // Nur genau diese Adressen: alles andere unter /api bleibt unberührt.
  worker = loadWorker({ cached: new Map([['/api/articles/7/images/3/x', stored]]) });
  for (const other of [
    '/api/articles/7/images/3/x', '/api/articles/7/images/', '/api/articles/7/images/abc',
    '/api/articles/abc/images/3', '/api/articles/7/highlights', '/api/articles/7',
    '/api/articles/7/refresh-images', '/x/api/articles/7/images/3',
  ]) {
    const request = get(other);
    const { intercepted } = await worker.run(request);
    // Die letzte Adresse liegt nicht unter /api und gehört der normalen Dateistrategie.
    assert.equal(intercepted, other.startsWith('/x/'), other);
  }

  // Nicht-GET-Anfragen auf die Bildadresse fasst er nie an.
  worker = loadWorker({ cached: new Map([[image, stored]]) });
  assert.equal((await worker.run({ method: 'DELETE', url: `https://leseliste.example${image}`, mode: 'cors' })).intercepted, false);
});

test('Beim Aktivieren bleibt der Bilder-Cache erhalten, alte Caches werden gelöscht', async () => {
  const worker = loadWorker({ cacheKeys: ['leseliste-shell-v0', 'leseliste-shell-v1', 'leseliste-images'] });
  await worker.activate();
  assert.deepEqual(worker.deleted, ['leseliste-shell-v0']);
});

test('Service Worker: Netz zuerst, bei Fehler oder Zeitlimit die gespeicherte Datei', async () => {
  // Netz antwortet: frische Datei, im Cache aufgefrischt
  const fresh = { ok: true, type: 'basic', tag: 'frisch', clone() { return this; } };
  let worker = loadWorker({ fetchImpl: async () => fresh });
  let result = await worker.run(get('/js/app.js'));
  assert.equal(result.response, fresh);
  assert.deepEqual(worker.puts, ['/js/app.js']);

  // Fehlerantworten werden nicht gespeichert
  worker = loadWorker({ fetchImpl: async () => ({ ok: false, type: 'basic', status: 404, clone() { return this; } }) });
  await worker.run(get('/js/fehlt.js'));
  assert.deepEqual(worker.puts, []);

  // Kein Netz: gespeicherte Datei
  const old = { tag: 'alt' };
  worker = loadWorker({
    fetchImpl: async () => { throw new TypeError('offline'); },
    cached: new Map([['/style.css', old]]),
  });
  assert.equal((await worker.run(get('/style.css'))).response, old);

  // Netz hängt: nach dem Zeitlimit kommt die gespeicherte Datei
  worker = loadWorker({
    fetchImpl: () => new Promise(() => {}),
    cached: new Map([['/js/api.js', old]]),
  });
  assert.equal((await worker.run(get('/js/api.js'))).response, old);

  // Seitenaufruf mit Suchparametern (Teilen): ein gemeinsamer Eintrag für die Startseite
  const page = { tag: 'startseite' };
  worker = loadWorker({
    fetchImpl: async () => { throw new TypeError('offline'); },
    cached: new Map([['/', page]]),
  });
  const share = get('/?url=https%3A%2F%2Fexample.org', { mode: 'navigate' });
  assert.equal((await worker.run(share)).response, page);
});

test('Die Dateiliste des Service Workers deckt alle App-Dateien ab', () => {
  const listed = [...read('sw.js').matchAll(/^\s+'(\/[^']*)',$/gm)].map((m) => m[1]);
  assert.ok(listed.includes('/'));
  const mustHave = [
    '/style.css',
    '/manifest.webmanifest',
    ...readdirSync(path.join(PUBLIC, 'js')).map((f) => `/js/${f}`),
    '/icons/icon-192.png',
    '/icons/icon-512.png',
  ];
  for (const file of mustHave) assert.ok(listed.includes(file), `${file} fehlt in SHELL`);
  for (const file of listed.filter((f) => f !== '/')) {
    assert.ok(existsSync(path.join(PUBLIC, file)), `${file} steht in SHELL, existiert aber nicht`);
  }
  assert.equal(listed.some((f) => f.startsWith('/api')), false);
});
