// Frontend-Ablauf: eigene Daten exportieren und in einem anderen Konto importieren.
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { ARTIKEL } from './fixtures.mjs';
import { setup, steps } from './helpers.mjs';

const { $, visible, until, site, teardown } = await setup({ '/artikel.html': ARTIKEL });
after(teardown);

async function register(email) {
  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  const form = $('form-auth');
  form.elements.email.value = email;
  form.elements.password.value = 'passwort-12345';
  form.requestSubmit();
  await until(() => visible('view-home'), 'Startseite');
}

/**
 * Sendet das Formular wie der Browser nach der Dateiauswahl. jsdom prüft `required` gegen die
 * interne (leere) Dateiliste, deshalb wird das submit-Ereignis direkt ausgelöst.
 */
function submitData() {
  $('form-data').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }));
}

/** Setzt die Dateiauswahl, wie es der Browser nach dem Auswählen tut. */
function choose(file) {
  Object.defineProperty($('form-data').elements.file, 'files', { value: [file], configurable: true });
}

test('Export herunterladen und in einem anderen Konto importieren', async (t) => {
  const ok = steps(t);

  // Konto A mit einem Artikel und Schlagwort
  await register('a@example.org');
  $('add-url').value = site.url('/artikel.html');
  $('form-add').requestSubmit();
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'Artikel gespeichert');
  const saved = (await (await fetch('/api/articles')).json())[0];
  const patch = await fetch(`/api/articles/${saved.id}`, {
    method: 'PATCH',
    headers: { 'X-Requested-With': 'fetch', 'Content-Type': 'application/json' },
    body: JSON.stringify({ tags: ['Test', 'Export'], is_read: true }),
  });
  assert.equal(patch.status, 200);
  const hl = await fetch(`/api/articles/${saved.id}/highlights`, {
    method: 'POST',
    headers: { 'X-Requested-With': 'fetch', 'Content-Type': 'application/json' },
    body: JSON.stringify({ start: 0, end: 4, quote: 'Dies', color: '#abcdef', note: 'Meine Notiz' }),
  });
  assert.equal(hl.status, 201);

  // Seite: Link ist ein normaler Download, keine Skripte oder Werkzeuge nötig
  window.location.hash = '#/account/data';
  await until(() => visible('view-data'), 'Seite Export und Import');
  assert.equal(document.title, 'Export und Import – Leseliste');
  const link = $('export-link');
  assert.equal(link.getAttribute('href'), '/api/export');
  assert.equal(link.hasAttribute('download'), true);
  assert.equal($('view-data').querySelector('a.button-link[href="#/"]') !== null, true);
  ok('Export ist ein einfacher Download-Link');

  // Was der Link liefert
  const response = await fetch(link.getAttribute('href'));
  assert.equal(response.status, 200);
  assert.match(response.headers.get('content-disposition'), /^attachment; filename="leseliste-export-\d{4}-\d\d-\d\d\.json"$/);
  const text = await response.text();
  const doc = JSON.parse(text);
  assert.equal(doc.format, 'leseliste-export');
  assert.equal(doc.articles.length, 1);
  assert.equal(doc.articles[0].title, 'E2E Testartikel');
  assert.deepEqual(doc.articles[0].tags, ['Export', 'Test']);
  assert.equal(doc.articles[0].is_read, true);
  assert.equal(doc.articles[0].highlights.length, 1);
  assert.equal(doc.articles[0].highlights[0].note, 'Meine Notiz');
  assert.equal(text.includes('a@example.org'), false, 'keine E-Mail-Adresse in der Datei');
  ok('Datei enthält Artikel mit Schlagwörtern und Status, aber keine Kontodaten');

  // Konto B importiert die Datei
  $('btn-logout').click();
  await register('b@example.org');
  window.location.hash = '#/account/data';
  await until(() => visible('view-data'), 'Seite im Konto B');
  assert.equal($('data-result').hidden, true);

  choose(new window.File([text], 'leseliste-export.json', { type: 'application/json' }));
  submitData();
  await until(() => visible('data-result'), 'Ergebnis des Imports');
  assert.equal($('data-result').textContent, 'Fertig: 1 importiert, 0 schon vorhanden, 1 Markierung.');
  assert.equal($('data-error').hidden, true);
  ok('Import meldet das Ergebnis');
  const imported = (await (await fetch('/api/articles')).json())[0];
  const importedHighlights = await (await fetch(`/api/articles/${imported.id}/highlights`)).json();
  assert.equal(importedHighlights.length, 1);
  assert.equal(importedHighlights[0].quote, 'Dies');
  assert.equal(importedHighlights[0].color, '#abcdef');
  assert.equal(importedHighlights[0].note, 'Meine Notiz');
  ok('Markierung und Notiz kommen im anderen Konto an');

  // Zweiter Import derselben Datei ändert nichts
  choose(new window.File([text], 'leseliste-export.json', { type: 'application/json' }));
  submitData();
  await until(() => /0 importiert/.test($('data-result').textContent), 'zweites Ergebnis');
  assert.equal($('data-result').textContent, 'Fertig: 0 importiert, 1 schon vorhanden.');
  ok('erneuter Import erzeugt keine Duplikate');

  // Die Liste zeigt den importierten Artikel samt Schlagwörtern
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'importierter Artikel');
  const item = document.querySelector('#article-list li');
  assert.equal(item.querySelector('.article-title').textContent, 'E2E Testartikel');
  assert.match(item.textContent, /Export/);
  ok('Liste zeigt den importierten Artikel');

  // Fehlerfälle
  window.location.hash = '#/account/data';
  await until(() => visible('view-data'), 'Seite erneut');
  choose(new window.File(['das ist kein json'], 'x.json', { type: 'application/json' }));
  submitData();
  await until(() => visible('data-error'), 'Fehler bei ungültiger Datei');
  assert.match($('data-error').textContent, /keine Exportdatei dieser App/);
  assert.equal($('data-result').hidden, true);

  const huge = new window.File(['{}'], 'riesig.json');
  Object.defineProperty(huge, 'size', { value: 65 * 1024 * 1024 });
  choose(huge);
  submitData();
  await until(() => /64 MiB/.test($('data-error').textContent), 'Fehler bei zu großer Datei');
  ok('ungültige und zu große Dateien werden mit Hinweis abgelehnt');
});
