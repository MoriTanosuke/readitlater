// Leseposition: wird beim Scrollen still gespeichert und beim erneuten Öffnen wiederhergestellt.
//
// jsdom hat kein Layout. Der Test stellt deshalb ein einfaches bereit: Der Artikeltext beginnt
// 300 px unter dem Seitenanfang, jeder der zwanzig Absätze ist 200 px hoch, das Fenster 800 px
// (das Textende ist also ab etwa Absatz 17 im Fenster zu sehen).
// Gescrollt wird, indem `scrollY` gesetzt und ein scroll-Ereignis ausgelöst wird (zuvor ein
// wheel-Ereignis, wie es ein Benutzer auslöst).
import { after, afterEach, test } from 'node:test';
import assert from 'node:assert/strict';

import { LANG } from './fixtures.mjs';
import { setup, sleep, steps } from './helpers.mjs';

const { $, visible, until, site, setOffline, teardown } = await setup({ '/lang.html': LANG });
after(teardown);

// Schlägt ein Test fehl, während die simulierte Verbindung weg ist oder hängt, würde das Aufräumen
// ewig auf offene Anfragen warten. Darum nach jedem Test die Verbindung wiederherstellen und
// jedem Test ein Zeitlimit geben: Ein Fehler soll als Fehler erscheinen, nicht die CI blockieren.
afterEach(() => {
  setOffline(false);
  extra = 0; // das simulierte Nachladen von Bildern soll Folgetests nicht beeinflussen
});
const spec = (name, fn) => test(name, { timeout: 60_000 }, fn);

const offline = await import('../public/js/offline.js');
const { api } = await import('../public/js/api.js');
const { textBlocks } = await import('../public/js/position.js');

// ---------------------------------------------------------------------------
// Simuliertes Layout
// ---------------------------------------------------------------------------

const DOC_TOP = 300; // Höhe über dem Text (Kopfzeile, Titel)
const BLOCK_H = 200;
const VIEWPORT_H = 800;
const LINE = 24; // Lesezeile unter dem Fensterrand (wie im Modul)
let scrollY = 0;
let extra = 0; // zusätzliche Höhe über dem Text, etwa durch nachgeladene Bilder

const root = () => $('reader-content');
const paragraphs = () => [...root().querySelectorAll('p')];
const maxScroll = () => DOC_TOP + extra + paragraphs().length * BLOCK_H + 400 - VIEWPORT_H;

function box(top, height) {
  return { top, bottom: top + height, left: 0, right: 600, width: 600, height, x: 0, y: top };
}

Object.defineProperty(window, 'scrollY', { configurable: true, get: () => scrollY });
window.innerHeight = VIEWPORT_H;
window.scrollTo = (first, second) => {
  const top = typeof first === 'object' ? first.top : second;
  scrollY = Math.max(0, Math.min(maxScroll(), top)); // wie im Browser: nie unter 0
};
window.Element.prototype.getBoundingClientRect = function getBoundingClientRect() {
  const view = $('view-reader');
  if (view.hidden || !view.contains(this)) return box(0, 0); // nicht dargestellt
  if (this === root()) return box(DOC_TOP + extra - scrollY, paragraphs().length * BLOCK_H);
  const index = paragraphs().indexOf(this);
  if (index >= 0) return box(DOC_TOP + extra + index * BLOCK_H - scrollY, BLOCK_H);
  return box(0, 0);
};

/** `scrollY`, bei dem Absatz `index` (0-basiert) zum Bruchteil `fraction` an der Lesezeile steht. */
const scrollYFor = (index, fraction) => DOC_TOP + extra + index * BLOCK_H + fraction * BLOCK_H - LINE;
/** Der Textoffset an dieser Stelle (wie ihn das Modul berechnet). */
function offsetFor(index, fraction) {
  const block = textBlocks(root())[index];
  return block.start + Math.floor(fraction * (block.end - block.start));
}
const near = (actual, expected, label) =>
  assert.ok(Math.abs(actual - expected) <= 2, `${label}: ${actual} statt ${expected}`);

/** Scrollt wie ein Benutzer. */
function userScroll(y) {
  window.dispatchEvent(new window.Event('wheel'));
  scrollY = y;
  window.dispatchEvent(new window.Event('scroll'));
}
const leavePage = () => window.dispatchEvent(new window.Event('pagehide'));

// ---------------------------------------------------------------------------
// Server und Hilfen
// ---------------------------------------------------------------------------

const rawFetch = globalThis.fetch; // ohne Aufzeichnung, für Eingriffe „von einem anderen Gerät“
const patches = []; // alle PATCH-Anfragen der App an Artikel
globalThis.fetch = (route, init = {}) => {
  if (init.method === 'PATCH' && /^\/api\/articles\/\d+$/.test(route)) {
    patches.push(JSON.parse(init.body));
  }
  return rawFetch(route, init);
};
const serverArticle = async (id) => (await rawFetch(`/api/articles/${id}`)).json();
const serverPos = async (id) => (await serverArticle(id)).read_pos;
const serverPatch = (id, body) =>
  rawFetch(`/api/articles/${id}`, {
    method: 'PATCH',
    headers: { 'X-Requested-With': 'fetch', 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });

async function untilAsync(check, label, timeout = 5000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    if (await check()) return;
    await sleep(40);
  }
  throw new Error(`Zeitüberschreitung: ${label}`);
}

/** Wartet, bis der Server die erwartete Position kennt (die Anfrage ist vielleicht noch unterwegs). */
const untilServerPos = (id, expected, label) =>
  untilAsync(async () => (await serverPos(id)) === expected, label);

async function openReader(id) {
  window.location.hash = `#/article/${id}`;
  await until(() => visible('view-reader') && root().textContent.length > 200, 'Lesemodus');
  await sleep(60); // Antwort des Servers abwarten
}

async function openList() {
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
}

let articleId = null;

spec('Position wird gespeichert, beim Öffnen wiederhergestellt und am Ende zurückgesetzt', async (t) => {
  const ok = steps(t);
  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  $('form-auth').elements.email.value = 'position@example.org';
  $('form-auth').elements.password.value = 'passwort-12345';
  $('form-auth').requestSubmit();
  await until(() => visible('view-home'), 'Startseite');
  $('add-url').value = site.url('/lang.html');
  $('form-add').requestSubmit();
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'Artikel gespeichert');
  articleId = Number(
    document.querySelector('#article-list .article-main').getAttribute('href').split('/').pop(),
  );

  // ---- Öffnen ohne Scrollen: oben, nichts wird gesendet
  await openReader(articleId);
  assert.equal(scrollY, 0);
  assert.equal((await serverArticle(articleId)).read_pos, 0);
  await sleep(1300);
  assert.equal(patches.length, 0, 'ohne Scrollen keine Anfrage');
  ok('neuer Artikel öffnet oben, ohne etwas zu senden');

  // ---- Scrollen: nach kurzer Pause gespeichert (höchstens einmal pro Sekunde)
  userScroll(scrollYFor(3, 0.25));
  userScroll(scrollYFor(3, 0.26)); // mehrere Ereignisse, eine Anfrage
  await untilAsync(() => patches.length === 1, 'Position gespeichert');
  assert.equal(patches.length, 1);
  assert.deepEqual(Object.keys(patches[0]), ['read_pos'], 'nur die Position, sonst nichts');
  near(patches[0].read_pos, offsetFor(3, 0.26), 'gespeicherte Position');
  assert.ok(patches[0].read_pos > offsetFor(2, 0.9), 'liegt im vierten Absatz');
  await untilServerPos(articleId, patches[0].read_pos, 'Server kennt die Position');
  ok('Scrollen speichert nach einer Pause genau eine Position (Absatz 4)');

  // ---- Verlassen der Ansicht speichert sofort
  userScroll(scrollYFor(6, 0.5));
  await openList();
  await untilAsync(() => patches.length === 2, 'beim Verlassen gespeichert', 1000);
  near(patches[1].read_pos, offsetFor(6, 0.5), 'Position beim Verlassen');
  await untilServerPos(articleId, patches[1].read_pos, 'Server kennt die Position beim Verlassen');
  ok('Zurück zur Liste speichert sofort, ohne die Pause abzuwarten');

  // ---- Wiederöffnen springt an die Stelle
  await openReader(articleId);
  near(scrollY, scrollYFor(6, 0.5), 'wiederhergestellte Stelle');
  assert.equal(patches.length, 2, 'Wiederherstellen sendet nichts');
  ok('erneutes Öffnen scrollt zur gemerkten Stelle (Absatz 7, Mitte)');

  // ---- Weiterlesen bis zum Ende: Position 0, beim nächsten Öffnen oben
  userScroll(maxScroll()); // Ende des Textes ist im Fenster
  leavePage();
  await untilAsync(() => patches.length === 3, 'Ende gespeichert', 1000);
  assert.deepEqual(patches[2], { read_pos: 0 });
  await untilServerPos(articleId, 0, 'Server kennt „Ende“');
  await openList();
  await openReader(articleId);
  assert.equal(scrollY, 0, 'zu Ende gelesen: wieder oben');
  ok('bis zum Ende gescrollt: Position 0, nächstes Öffnen beginnt oben');

  // ---- Zurückscrollen nach dem Ende zählt wieder (Position = wo man jetzt steht)
  userScroll(maxScroll());
  leavePage();
  await sleep(100);
  patches.length = 0;
  userScroll(scrollYFor(4, 0));
  leavePage();
  await untilAsync(() => patches.length === 1, 'Position nach dem Ende');
  near(patches[0].read_pos, offsetFor(4, 0), 'Position nach dem Zurückscrollen');
  assert.ok(patches[0].read_pos > 0);
  ok('nach dem Ende wieder hochgescrollt: die aktuelle Stelle wird gemerkt');
  await openList();

  // ---- Ganz nach oben scrollen speichert 0, nicht einen kleinen Rest
  await openReader(articleId);
  userScroll(0);
  leavePage();
  await untilAsync(async () => (await serverPos(articleId)) === 0, 'oben gespeichert');
  ok('zurück am Artikelanfang: Position 0');
  await openList();
});

spec('Gelesen markieren setzt zurück und hält die Aufzeichnung an', async (t) => {
  const ok = steps(t);
  await openReader(articleId);
  patches.length = 0;
  userScroll(scrollYFor(2, 0.5));
  leavePage();
  await untilAsync(() => patches.length === 1, 'Position vor „gelesen“');
  await untilAsync(async () => (await serverPos(articleId)) > 0, 'Server kennt die Position vor „gelesen“');

  $('reader-toggle-read').click();
  await untilAsync(() => patches.some((p) => p.is_read === true), '„gelesen“ gesendet');
  await untilAsync(async () => (await serverArticle(articleId)).is_read === true, 'Server: gelesen');
  assert.equal(await serverPos(articleId), 0, 'gelesen: Position zurückgesetzt');

  // Weiterscrollen im gelesenen Artikel sendet nichts mehr
  const before = patches.length;
  userScroll(scrollYFor(5, 0));
  leavePage();
  await sleep(1300);
  assert.equal(patches.length, before, 'gelesene Artikel merken keine Position');
  assert.equal(await serverPos(articleId), 0);
  ok('als gelesen markiert: Position 0 und keine weiteren Anfragen');

  // Neu öffnen: oben
  await openList();
  await openReader(articleId);
  assert.equal(scrollY, 0, 'gelesener Artikel beginnt oben');

  // Wieder ungelesen: die Aufzeichnung läuft weiter
  $('reader-toggle-read').click();
  await untilAsync(async () => (await serverArticle(articleId)).is_read === false, 'Server: ungelesen');
  patches.length = 0;
  userScroll(scrollYFor(8, 0));
  leavePage();
  await untilAsync(() => patches.length === 1, 'Position nach „ungelesen“');
  near(patches[0].read_pos, offsetFor(8, 0), 'Position nach „ungelesen“');
  ok('wieder ungelesen: Position wird erneut gemerkt');
  await openList();
});

spec('Eine neuere Position vom Server gewinnt, solange der Benutzer nichts getan hat', async (t) => {
  const ok = steps(t);
  // Stand dieses Geräts: Absatz 9. Auf einem anderen Gerät wurde bis Absatz 3 weitergelesen.
  await serverPatch(articleId, { read_pos: offsetFor(2, 0) });
  await openReader(articleId);
  await untilAsync(() => Math.abs(scrollY - scrollYFor(2, 0)) <= 2, 'Position vom Server übernommen');
  ok('Server-Position ersetzt die ältere Kopie auf dem Gerät');

  // Hat der Benutzer schon gescrollt, bleibt seine Stelle
  await openList();
  await serverPatch(articleId, { read_pos: offsetFor(7, 0) });
  setOffline('hang'); // der Server antwortet erst später
  window.location.hash = `#/article/${articleId}`;
  await until(() => visible('view-reader') && root().textContent.length > 200, 'Lesemodus aus Kopie');
  near(scrollY, scrollYFor(2, 0), 'Stelle aus der Kopie');
  userScroll(scrollYFor(1, 0));
  setOffline(false); // hängende Anfragen scheitern, die App bleibt bei ihrer Stelle
  await sleep(300);
  near(scrollY, scrollYFor(1, 0), 'Benutzer behält seine Stelle');
  leavePage();
  await openList();
  ok('bereits gescrollt: die Server-Antwort verschiebt nichts mehr');
});

spec('Nachladende Bilder verschieben den Text: die Stelle wird nachgeführt', async (t) => {
  const ok = steps(t);
  await serverPatch(articleId, { read_pos: offsetFor(5, 0) });
  await openReader(articleId);
  await untilAsync(() => Math.abs(scrollY - scrollYFor(5, 0)) <= 2, 'Stelle wiederhergestellt');

  // Ein Bild über der Stelle lädt und schiebt den Text 100 px nach unten.
  extra = 100;
  paragraphs()[0].dispatchEvent(new window.Event('load'));
  await untilAsync(() => Math.abs(scrollY - scrollYFor(5, 0)) <= 2, 'Stelle nachgeführt');
  assert.ok(scrollY > DOC_TOP + 5 * BLOCK_H, 'weiter unten');
  ok('Bild lädt, die gemerkte Stelle bleibt im Blick');

  // Nach einer Eingabe des Benutzers wird nichts mehr verschoben.
  window.dispatchEvent(new window.Event('wheel'));
  const stay = scrollY;
  extra = 250;
  paragraphs()[0].dispatchEvent(new window.Event('load'));
  await sleep(100);
  assert.equal(scrollY, stay, 'nach Eingabe bleibt der Benutzer, wo er ist');
  extra = 0;
  ok('nach einer Eingabe wird die Ansicht nicht mehr verschoben');
  await openList();
});

spec('Nachladende Webfonts verschieben den Text: die Stelle wird nachgeführt', async (t) => {
  const ok = steps(t);
  // jsdom kennt document.fonts nicht: ein EventTarget stellt das Ereignis `loadingdone` nach.
  const fonts = new window.EventTarget();
  Object.defineProperty(window.document, 'fonts', { value: fonts, configurable: true });
  try {
    await serverPatch(articleId, { read_pos: offsetFor(5, 0) });
    await openReader(articleId);
    await untilAsync(() => Math.abs(scrollY - scrollYFor(5, 0)) <= 2, 'Stelle wiederhergestellt');

    // Die Schrift lädt nach, der Text wird 80 px höher und schiebt die Stelle nach unten.
    extra = 80;
    fonts.dispatchEvent(new window.Event('loadingdone'));
    await untilAsync(() => Math.abs(scrollY - scrollYFor(5, 0)) <= 2, 'Stelle nachgeführt');
    ok('Schrift lädt nach, die gemerkte Stelle bleibt im Blick');

    // Nach einer Eingabe des Benutzers wird nichts mehr verschoben.
    window.dispatchEvent(new window.Event('wheel'));
    const stay = scrollY;
    extra = 200;
    fonts.dispatchEvent(new window.Event('loadingdone'));
    await sleep(100);
    assert.equal(scrollY, stay, 'nach Eingabe bleibt der Benutzer, wo er ist');
    ok('nach einer Eingabe wird die Ansicht nicht mehr verschoben');
  } finally {
    extra = 0;
    delete window.document.fonts;
    await openList();
  }
});

spec('Ohne Verbindung wird die Position vorgemerkt und beim Abgleich gesendet', async (t) => {
  const ok = steps(t);
  await serverPatch(articleId, { read_pos: 0 });
  await openReader(articleId); // legt die Kopie auf dem Gerät an
  await openList();

  // ---- Offline lesen: Position landet in der Kopie und in der Warteschlange
  setOffline(true);
  await openReader(articleId);
  assert.equal(scrollY, 0);
  userScroll(scrollYFor(4, 0.5));
  leavePage();
  const wanted = offsetFor(4, 0.5);
  await untilAsync(async () => (await offline.getArticle(articleId))?.read_pos === wanted, 'Kopie aktualisiert');
  assert.equal(await serverPos(articleId).catch(() => -1), -1, 'der Server ist nicht erreichbar');
  await openList();

  // Auch ohne Verbindung beginnt der nächste Besuch an der Stelle.
  await openReader(articleId);
  near(scrollY, scrollYFor(4, 0.5), 'Stelle ohne Verbindung');
  await openList();

  // ---- Verbindung zurück: der Abgleich sendet die vorgemerkte Position
  setOffline(false);
  const result = await offline.sync(api);
  assert.ok(result.flushed >= 1, 'vorgemerkte Änderung gesendet');
  assert.equal(await serverPos(articleId), wanted);
  ok('offline gemerkt, nach der Rückkehr zum Server übertragen');

  // ---- Eine neuere, direkt gesendete Position ersetzt die vorgemerkte
  setOffline(true);
  await openReader(articleId);
  userScroll(scrollYFor(1, 0));
  leavePage();
  await untilAsync(async () => (await offline.getArticle(articleId))?.read_pos === offsetFor(1, 0), 'zweite vorgemerkt');
  await openList();
  setOffline(false);
  await openReader(articleId); // online: Server kennt noch die alte Position
  patches.length = 0;
  userScroll(scrollYFor(7, 0));
  leavePage();
  await untilAsync(async () => (await serverPos(articleId)) === offsetFor(7, 0), 'Server aktuell');
  const again = await offline.sync(api);
  assert.equal(again.flushed, 0, 'nichts mehr vorgemerkt');
  assert.equal(await serverPos(articleId), offsetFor(7, 0), 'Server behält die neuere Position');
  ok('die direkt gesendete Position überschreibt keine alte Vormerkung');
  await openList();
});

spec('Läuft die Sitzung beim Lesen ab, geht es zur Anmeldung, ohne weitere Anfragen', async (t) => {
  const ok = steps(t);
  await openReader(articleId);
  userScroll(scrollYFor(3, 0));
  // Die Sitzung endet auf dem Server (z. B. abgelaufen), während der Benutzer liest.
  await rawFetch('/api/logout', { method: 'POST', headers: { 'X-Requested-With': 'fetch' } });
  leavePage(); // das Speichern scheitert mit 401
  await until(() => visible('view-auth'), 'Anmeldemaske nach abgelaufener Sitzung');
  patches.length = 0;
  await sleep(1300); // die Pause des Speicherns darf nichts mehr auslösen
  assert.equal(patches.length, 0, 'nach dem Ende der Sitzung keine Anfrage');
  ok('abgelaufene Sitzung: Anmeldemaske, keine weiteren Anfragen');
});
