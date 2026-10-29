// Offline-Lesen: Artikel werden auf dem Gerät gespeichert und ohne Verbindung angezeigt.
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { ARTIKEL, KAFFEE } from './fixtures.mjs';
import { setup, sleep, steps } from './helpers.mjs';

const { $, visible, until, site, setOffline, teardown } = await setup({
  '/artikel.html': ARTIKEL,
  '/kaffee.html': KAFFEE,
});
after(teardown);

const rows = () => [...document.querySelectorAll('#article-list li')];
const titles = () => rows().map((li) => li.querySelector('.article-title').textContent);
const json = async (path) => (await fetch(path)).json();
const send = (path, method, body) =>
  fetch(path, {
    method,
    headers: { 'X-Requested-With': 'fetch', 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });

/** Wartet auf eine Bedingung, die erst eine Anfrage an den Server beantwortet. */
async function untilAsync(check, label, timeout = 8000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    if (await check()) return;
    await sleep(50);
  }
  throw new Error(`Zeitüberschreitung: ${label}`);
}

async function register(email) {
  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  $('form-auth').elements.email.value = email;
  $('form-auth').elements.password.value = 'passwort-12345';
  $('form-auth').requestSubmit();
  await until(() => visible('view-home'), 'Startseite');
}

async function save(url, count) {
  $('add-url').value = site.url(url);
  $('form-add').requestSubmit();
  await until(() => rows().length === count, `Artikel ${count} gespeichert`);
}

test('Artikel offline lesen, durchsuchen, als gelesen vormerken und abmelden', async (t) => {
  const ok = steps(t);
  const offline = await import('../public/js/offline.js');

  await register('zug@example.org');
  await save('/artikel.html', 1);
  await save('/kaffee.html', 2);
  const all = await json('/api/articles');
  const artikel = all.find((a) => a.title === 'E2E Testartikel');
  const kaffee = all.find((a) => a.title === 'Kaffee und Bohnen');
  await send(`/api/articles/${kaffee.id}`, 'PATCH', { tags: ['Genuss'] });
  const hl = await send(`/api/articles/${artikel.id}/highlights`, 'POST', {
    start: 0, end: 4, quote: 'Dies', color: '#abcdef', note: 'Meine Notiz',
  });
  assert.equal(hl.status, 201);

  // Der Abgleich im Hintergrund startet beim ersten Laden der Liste und läuft nebenher.
  // Sieht er einen Artikel, bevor die Markierung existiert, bleibt dessen gespeicherte
  // Fassung bei späteren Abgleichen unverändert (Markierungen kommen nur beim ersten
  // Speichern mit). Ob er das tut, hängt vom Zeitablauf ab, auf einem langsamen Rechner
  // (CI) also mal ja, mal nein. Deshalb: den Abgleich abwarten und den Speicher leeren,
  // damit die Aktualisierung unten immer alles frisch mit Markierung holt.
  const { api } = await import('../public/js/api.js');
  await offline.sync(api);
  await offline.remove(artikel.id);
  await offline.remove(kaffee.id);
  assert.equal(await offline.count(), 0);

  // ---- Einstellungsseite: „Jetzt aktualisieren“ speichert die ungelesenen Artikel
  window.location.hash = '#/account/offline';
  await until(() => visible('view-offline'), 'Seite Offline-Lesen');
  await until(() => /noch nichts gespeichert|gespeichert: \d/.test($('offline-status').textContent), 'Status');
  assert.equal($('form-offline').elements.enabled.checked, true, 'standardmäßig an');
  $('form-offline').requestSubmit();
  await until(() => /gespeichert: 2 Artikel/.test($('offline-status').textContent), 'zwei Artikel gespeichert');
  assert.equal(await offline.count(), 2);
  const stored = await offline.getArticle(artikel.id);
  assert.match(stored.content, /ausführlicher Absatz/);
  assert.equal(stored.highlights.length, 1);
  assert.equal(stored.highlights[0].note, 'Meine Notiz');
  assert.equal(stored.content.includes('<script'), false);
  ok('Abgleich speichert ungelesene Artikel samt Markierungen');

  // ---- Verbindung weg: Liste kommt aus dem Gerätespeicher
  setOffline(true);
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  $('form-filter').requestSubmit(); // lädt die Liste neu, wie nach dem Öffnen der App
  await until(() => visible('offline-banner') && rows().length === 2, 'Liste ohne Verbindung');
  assert.deepEqual(titles().sort(), ['E2E Testartikel', 'Kaffee und Bohnen']);
  assert.equal($('list-error').hidden, true, 'kein Fehler, nur der Hinweis');
  assert.equal($('btn-more').hidden, true);
  await until(() => document.querySelectorAll('#tag-bar .tag-chip').length === 1, 'Schlagwörter offline');
  document.querySelector('#tag-bar .tag-chip').click();
  await until(() => rows().length === 1, 'Filter nach Schlagwort');
  assert.deepEqual(titles(), ['Kaffee und Bohnen']);
  document.querySelector('#tag-bar .tag-chip').click();
  await until(() => rows().length === 2, 'Filter aufgehoben');
  $('filter-q').value = 'kaffée'; // Akzente zählen nicht, wie bei der Suche im Backend
  $('form-filter').requestSubmit();
  await until(() => rows().length === 1 && titles()[0] === 'Kaffee und Bohnen', 'Suche offline');
  $('filter-q').value = 'gibtesnicht';
  $('form-filter').requestSubmit();
  await until(() => rows().length === 0 && visible('list-empty'), 'keine Treffer');
  assert.match($('list-empty').textContent, /gespeicherten Artikel/);
  $('filter-q').value = '';
  $('form-filter').requestSubmit();
  await until(() => rows().length === 2, 'Liste wieder vollständig');
  ok('Liste, Schlagwörter und Suche ohne Verbindung');

  // ---- Artikel lesen, inklusive Markierung und Notiz
  window.location.hash = `#/article/${artikel.id}`;
  await until(() => visible('view-reader') && $('reader-content').children.length > 0, 'Lesemodus offline');
  assert.equal($('reader-title').textContent, 'E2E Testartikel');
  assert.match($('reader-content').textContent, /ausführlicher Absatz/);
  assert.equal($('reader-content').querySelector('script, [onclick]'), null);
  await until(() => $('reader-content').querySelector('mark.hl'), 'Markierung offline');
  assert.equal($('reader-content').querySelector('mark.hl').textContent, 'Dies');
  assert.match($('reader-notes').textContent, /Meine Notiz/);
  await until(() => visible('offline-banner'), 'Hinweis im Lesemodus');
  assert.equal($('reader-error').hidden, true);
  ok('Artikel mit Markierung und Notiz offline lesbar');

  // ---- Änderungen: nur „gelesen“ lässt sich vormerken, alles andere meldet den Fehler
  $('tags-input').value = 'neu';
  $('form-tags').requestSubmit();
  await until(() => visible('tags-error'), 'Fehler bei Schlagwörtern');
  assert.match($('tags-error').textContent, /nicht erreichbar/);
  $('reader-toggle-read').click();
  await until(() => visible('reader-notice'), 'Hinweis vorgemerkt');
  assert.match($('reader-notice').textContent, /vorgemerkt/);
  assert.equal($('reader-toggle-read').textContent, 'Als ungelesen markieren');
  assert.equal((await offline.getArticle(artikel.id)).is_read, true, 'auch die gespeicherte Kopie');
  ok('Gelesen-Status offline vorgemerkt, andere Änderungen melden den Fehler');

  // ---- Ein Artikel, der nicht gespeichert ist, lässt sich offline nicht öffnen
  window.location.hash = '#/article/99999';
  await until(() => visible('reader-error'), 'Fehler für nicht gespeicherten Artikel');
  assert.match($('reader-error').textContent, /nicht gespeichert/);
  assert.equal($('reader-content').children.length, 0);
  ok('nicht gespeicherter Artikel zeigt eine klare Meldung');

  // ---- Abmelden braucht eine Verbindung, die Sitzung soll wirklich enden
  window.location.hash = '#/';
  await until(() => visible('view-home') && rows().length === 2, 'Liste');
  $('btn-logout').click();
  await until(() => visible('list-error'), 'Hinweis zum Abmelden');
  assert.match($('list-error').textContent, /Abmelden geht nur mit Internetverbindung/);
  assert.equal(visible('view-home'), true, 'bleibt angemeldet');
  assert.equal(await offline.count(), 2, 'gespeicherte Artikel bleiben');
  ok('Abmelden ohne Verbindung wird abgelehnt');

  // ---- Verbindung zurück: vorgemerkte Änderung wird gesendet, Hinweis verschwindet
  setOffline(false);
  window.dispatchEvent(new window.Event('online'));
  await untilAsync(async () => (await json(`/api/articles/${artikel.id}`)).is_read === true, 'Gelesen-Status gesendet');
  await until(() => !visible('offline-banner'), 'Hinweis weg');
  await until(() => rows().length === 2 && !visible('list-error'), 'Liste vom Server');
  ok('nach der Rückkehr der Verbindung wird abgeglichen');

  // ---- Abmelden löscht alles auf dem Gerät
  $('btn-logout').click();
  await until(() => visible('view-auth'), 'abgemeldet');
  assert.equal(await offline.count(), 0, 'Gerätespeicher leer');
  assert.equal(await offline.getOwner(), null);
  ok('Abmelden löscht die gespeicherten Artikel');

  // ---- Anderer Benutzer: nichts vom vorherigen bleibt übrig
  await offline.putArticle({
    id: 1, url: 'https://example.org/', title: 'Fremd', excerpt: '', content: '<p>x</p>',
    is_read: false, created_at: '2026-01-01T00:00:00Z', tags: [], highlights: [],
  });
  await offline.adopt({ id: 1, email: 'a@example.org' });
  assert.equal(await offline.count(), 1);
  await offline.adopt({ id: 2, email: 'b@example.org' });
  assert.equal(await offline.count(), 0, 'Daten eines anderen Benutzers werden verworfen');
  assert.equal((await offline.getOwner()).id, 2);
  await offline.clearAll();
  ok('Daten eines anderen Benutzers werden verworfen');
});

test('Einstellung: ausschalten löscht den Speicher, einschalten lädt neu', async (t) => {
  const ok = steps(t);
  const offline = await import('../public/js/offline.js');

  $('tab-login').click();
  $('form-auth').elements.email.value = 'zug@example.org';
  $('form-auth').elements.password.value = 'passwort-12345';
  $('form-auth').requestSubmit();
  await until(() => visible('view-home') && rows().length === 2, 'angemeldet');

  window.location.hash = '#/account/offline';
  await until(() => visible('view-offline'), 'Seite');
  $('form-offline').requestSubmit();
  await until(() => /gespeichert: \d+ Artikel/.test($('offline-status').textContent), 'gespeichert');
  assert.ok((await offline.count()) >= 1);

  const checkbox = $('form-offline').elements.enabled;
  checkbox.checked = false;
  checkbox.dispatchEvent(new window.Event('change', { bubbles: true }));
  await until(() => /ausgeschaltet/.test($('offline-status').textContent), 'ausgeschaltet');
  assert.equal(await offline.count(), 0);
  assert.equal(offline.isEnabled(), false);
  assert.equal($('offline-sync').disabled, true);
  ok('Ausschalten löscht die gespeicherten Artikel');

  checkbox.checked = true;
  checkbox.dispatchEvent(new window.Event('change', { bubbles: true }));
  await until(() => /gespeichert: \d+ Artikel/.test($('offline-status').textContent), 'wieder gespeichert');
  assert.ok((await offline.count()) >= 1);
  $('offline-clear').click();
  await until(() => /noch nichts gespeichert/.test($('offline-status').textContent), 'gelöscht');
  assert.equal(await offline.count(), 0);
  ok('Einschalten lädt neu, „Löschen“ leert den Speicher');
});

test('Bei hängender Verbindung erscheinen gespeicherte Artikel, ohne zu warten', async (t) => {
  const ok = steps(t);
  const offline = await import('../public/js/offline.js');

  window.location.hash = '#/account/offline';
  await until(() => visible('view-offline'), 'Seite');
  $('form-offline').requestSubmit();
  await until(() => /gespeichert: \d+ Artikel/.test($('offline-status').textContent), 'gespeichert');
  const [first] = await offline.list({});
  assert.ok(first);

  setOffline('hang'); // Anfragen kommen nie an, wie bei sehr schlechtem Empfang
  window.location.hash = `#/article/${first.id}`;
  await until(() => visible('view-reader') && $('reader-content').children.length > 0, 'Artikel aus dem Speicher');
  assert.equal($('reader-title').textContent, first.title);
  ok('Lesemodus zeigt die gespeicherte Fassung sofort');

  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  $('form-filter').requestSubmit(); // Liste neu laden: der Server antwortet nie
  assert.equal(rows().length, 0, 'zunächst wartet die Liste auf den Server');
  await until(() => rows().length >= 1 && visible('offline-banner'), 'gespeicherte Liste nach kurzer Wartezeit', 6000);
  ok('Liste zeigt nach kurzer Wartezeit die gespeicherten Artikel');

  setOffline(false);
  window.dispatchEvent(new window.Event('online'));
  await until(() => !visible('offline-banner') && rows().length >= 1, 'Verbindung zurück');
});
