// Leseeinstellungen: Schriftart, Größe, Breite, Zeilenabstand, Farbschema und Kontrast.
import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';
import path from 'node:path';

import { ARTIKEL, ENGLISCH } from './fixtures.mjs';
import { PUBLIC, setup, steps } from './helpers.mjs';

const { $, visible, until, site, teardown } = await setup({
  '/artikel.html': ARTIKEL,
  '/english.html': ENGLISCH,
});
after(teardown);

const reading = await import(pathToFileURL(path.join(PUBLIC, 'js/reading.js')).href);
const root = () => document.documentElement;
const stored = () => window.localStorage.getItem('leseliste-reading');

/** Setzt ein Bedienelement wie ein Benutzer (change bei select, input bei Schiebereglern). */
function set(container, index, value, type) {
  const control = $(container).querySelectorAll('select, input[type="range"]')[index];
  control.value = String(value);
  control.dispatchEvent(new window.Event(type, { bubbles: true }));
  return control;
}
// Reihenfolge der Bedienelemente
const FONT = 0, SIZE = 1, WIDTH = 2, LEADING = 3, ALIGN = 4, THEME = 5, CONTRAST = 6;

test('normalize macht aus kaputten Daten gültige Einstellungen', () => {
  assert.deepEqual(reading.normalize(null), reading.DEFAULTS);
  assert.deepEqual(reading.normalize('x'), reading.DEFAULTS);
  assert.deepEqual(reading.normalize([1, 2]), reading.DEFAULTS);
  const kaputt = reading.normalize({
    font: '__proto__', size: 'groß', width: Infinity, leading: NaN, theme: 'neon', contrast: 5,
  });
  assert.deepEqual(kaputt, reading.DEFAULTS);
  assert.equal(reading.normalize({ align: 'center' }).align, 'left');
  const grenzen = reading.normalize({ size: 5000, width: -3, leading: 99, font: 'serif' });
  assert.deepEqual(grenzen, { ...reading.DEFAULTS, size: 200, width: 40, leading: 2.2, font: 'serif' });
  // Zwischenwerte rasten auf die Schrittweite ein.
  assert.equal(reading.normalize({ size: 133 }).size, 130);
  assert.equal(reading.normalize({ leading: 1.74 }).leading, 1.7);
});

test('Seite „Darstellung“ und Bedienfeld im Lesemodus', async (t) => {
  const ok = steps(t);
  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  const auth = $('form-auth');
  auth.elements.email.value = 'lesen@example.org';
  auth.elements.password.value = 'passwort-12345';
  auth.requestSubmit();
  await until(() => visible('view-home'), 'Startseite');

  // Voreinstellung
  assert.equal(root().dataset.theme, 'auto');
  assert.equal(root().dataset.contrast, 'normal');
  assert.equal(root().dataset.readingFont, 'sans');
  assert.equal(stored(), null, 'Voreinstellung wird nicht gespeichert');
  ok('Voreinstellung: Gerät, normaler Kontrast, Systemschrift, nichts gespeichert');

  // Seite über das Menü
  assert.ok($('user-menu').querySelector('a[href="#/account/display"]'));
  window.location.hash = '#/account/display';
  await until(() => visible('view-display'), 'Seite Darstellung');
  assert.equal($('display-controls').querySelectorAll('select, input[type="range"]').length, 7);
  assert.equal($('display-controls').querySelector('.reading-reset').disabled, true);

  set('display-controls', FONT, 'serif', 'change');
  assert.equal(root().dataset.readingFont, 'serif');
  set('display-controls', SIZE, 150, 'input');
  assert.equal(root().style.getPropertyValue('--reading-size'), `${(1.0625 * 150) / 100}rem`);
  set('display-controls', WIDTH, 55, 'input');
  assert.equal(root().style.getPropertyValue('--reading-measure'), '55');
  set('display-controls', LEADING, 2, 'input');
  assert.equal(root().style.getPropertyValue('--reading-leading'), '2');
  set('display-controls', ALIGN, 'justify', 'change');
  assert.equal(root().dataset.readingAlign, 'justify');
  set('display-controls', THEME, 'dark', 'change');
  assert.equal(root().dataset.theme, 'dark');
  set('display-controls', CONTRAST, 'high', 'change');
  assert.equal(root().dataset.contrast, 'high');
  ok('alle sieben Einstellungen wirken sofort auf das Dokument');

  // Anzeige der Werte und Speicherung
  const outputs = [...$('display-controls').querySelectorAll('output')].map((o) => o.textContent);
  assert.deepEqual(outputs, ['150 %', '55 Zeichen', '2,0']);
  assert.deepEqual(JSON.parse(stored()), {
    font: 'serif', size: 150, width: 55, leading: 2, align: 'justify', theme: 'dark', contrast: 'high',
  });
  assert.equal($('display-controls').querySelector('.reading-reset').disabled, false);
  ok('Werte werden angezeigt und pro Gerät gespeichert');

  // Ungültige Eingaben werden begrenzt
  set('display-controls', SIZE, 9999, 'input');
  assert.equal(reading.getSettings().size, 200);

  // Artikel öffnen: Bedienfeld „Aa“
  $('add-url').value = site.url('/artikel.html');
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  $('form-add').requestSubmit();
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'Artikel');
  window.location.hash = document.querySelector('#article-list .article-main').getAttribute('href');
  await until(() => visible('view-reader') && $('reader-content').textContent.length > 20, 'Lesemodus');

  assert.equal($('reading-panel').hidden, true);
  assert.equal($('reading-toggle').getAttribute('aria-expanded'), 'false');
  $('reading-toggle').click();
  assert.equal($('reading-panel').hidden, false);
  assert.equal($('reading-toggle').getAttribute('aria-expanded'), 'true');
  // Das Bedienfeld zeigt die gespeicherten Werte
  assert.equal($('reading-panel').querySelectorAll('select')[0].value, 'serif');
  assert.equal($('reading-panel').querySelector('input[type="range"]').value, '200');
  // Änderung im Lesemodus gleicht die Seite „Darstellung“ an
  set('reading-panel', FONT, 'mono', 'change');
  assert.equal(root().dataset.readingFont, 'mono');
  assert.equal($('display-controls').querySelectorAll('select')[0].value, 'mono');
  ok('Bedienfeld im Lesemodus öffnet, zeigt und ändert dieselben Einstellungen');

  // Der Text sitzt in der Lesespalte, Bedienelemente nicht im Text
  assert.ok($('reader-content').closest('.reading-scope'));

  // Escape schließt, Navigation schließt
  document.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  assert.equal($('reading-panel').hidden, true);
  $('reading-toggle').click();
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  assert.equal($('reading-panel').hidden, true);
  assert.equal($('reading-toggle').getAttribute('aria-expanded'), 'false');
  ok('Escape und Navigation schließen das Bedienfeld');

  // Zurücksetzen
  window.location.hash = '#/account/display';
  await until(() => visible('view-display'), 'Seite Darstellung');
  $('display-controls').querySelector('.reading-reset').click();
  assert.deepEqual(reading.getSettings(), reading.DEFAULTS);
  assert.equal(root().dataset.theme, 'auto');
  assert.equal(root().style.getPropertyValue('--reading-measure'), '70');
  assert.equal(stored(), null);
  assert.equal($('display-controls').querySelector('.reading-reset').disabled, true);
  ok('Zurücksetzen stellt die Voreinstellung her und löscht den Speicher');

  // Änderung aus einem anderen Tab (storage-Ereignis) und kaputter Speicher
  window.localStorage.setItem('leseliste-reading', JSON.stringify({ font: 'wide', contrast: 'soft' }));
  window.dispatchEvent(new window.StorageEvent('storage', { key: 'leseliste-reading' }));
  assert.equal(root().dataset.readingFont, 'wide');
  assert.equal(root().dataset.contrast, 'soft');
  window.localStorage.setItem('leseliste-reading', '{kaputt');
  window.dispatchEvent(new window.StorageEvent('storage', { key: 'leseliste-reading' }));
  assert.deepEqual(reading.getSettings(), reading.DEFAULTS);
  ok('andere Tabs und kaputte Daten werden sicher verarbeitet');
});

test('Sprache des Artikels steht als lang-Attribut im Lesemodus', async (t) => {
  const ok = steps(t);
  // Der Test davor hat sich angemeldet; hier zwei Artikel speichern und lesen.
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  for (const route of ['/english.html']) {
    $('add-url').value = site.url(route);
    $('form-add').requestSubmit();
  }
  await until(() => document.querySelectorAll('#article-list li').length === 2, 'zwei Artikel');
  const open = async (title) => {
    const item = [...document.querySelectorAll('#article-list li')].find((li) =>
      li.querySelector('.article-title').textContent === title);
    window.location.hash = item.querySelector('.article-main').getAttribute('href');
    await until(() => visible('view-reader') && $('reader-title').textContent === title, title);
  };
  await open('English Article');
  assert.equal($('reader-content').lang, 'en');
  assert.equal($('reader-title').lang, 'en');
  ok('englischer Text: lang="en", obwohl die Seite lang="de" behauptet');
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  await open('E2E Testartikel');
  assert.equal($('reader-content').lang, 'de');
  ok('deutscher Text: lang="de"');
});
