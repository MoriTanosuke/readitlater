// Frontend-Ablauf: Suche mit Markierung, Gelesen-Filter, Schlagwörter, Schlagwortleiste.
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { ARTIKEL, KAFFEE } from './fixtures.mjs';
import { setup, sleep, steps } from './helpers.mjs';

const { $, visible, until, site, teardown } = await setup({
  '/artikel.html': ARTIKEL,
  '/zweiter.html': KAFFEE,
});
after(teardown);
const SITE = site.url('/artikel.html');
const SITE2 = site.url('/zweiter.html');

test('Suche, Gelesen-Status und Schlagwörter', async (t) => {
  const ok = steps(t);

  const items = () => [...document.querySelectorAll('#article-list li')];
  const titles = () => items().map((li) => li.querySelector('.article-title').textContent);
  const type = (el, value) => {
    el.value = value;
    el.dispatchEvent(new window.Event('input', { bubbles: true }));
  };

  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  const form = $('form-auth');
  form.elements.email.value = 'e2e2@example.org';
  form.elements.password.value = 'passwort-12345';
  form.requestSubmit();
  await until(() => visible('view-home'), 'Startseite');
  assert.equal($('tag-bar').hidden, true, 'ohne Schlagwörter keine Leiste');

  for (const url of [SITE, SITE2]) {
    $('add-url').value = url;
    $('form-add').requestSubmit();
    const n = items().length + 1;
    await until(() => items().length === n, `Artikel ${url}`);
  }
  assert.deepEqual(titles(), ['Kaffee und Bohnen', 'E2E Testartikel']);
  ok('zwei Artikel gespeichert');

  // Suche (mit Verzögerung beim Tippen)
  type($('filter-q'), 'kaffeeboh');
  await until(() => titles().length === 1, 'Suchergebnis');
  assert.deepEqual(titles(), ['Kaffee und Bohnen']);
  const mark = items()[0].querySelector('.article-excerpt mark');
  assert.ok(mark, 'Treffer ist markiert');
  assert.match(mark.textContent, /^Kaffeebohnen/i);
  assert.equal(items()[0].querySelector('.article-excerpt').textContent.includes('\u0001'), false);
  type($('filter-q'), 'gibtesnicht');
  await until(() => titles().length === 0 && visible('list-empty'), 'keine Treffer');
  assert.match($('list-empty').textContent, /Keine passenden/);
  type($('filter-q'), '"( OR');
  await sleep(500);
  assert.equal(visible('list-error'), false, 'Sonderzeichen erzeugen keinen Fehler');
  type($('filter-q'), '');
  await until(() => titles().length === 2, 'Suche zurückgesetzt');
  ok('Suche findet, markiert, zeigt Leerzustand und verträgt Sonderzeichen');

  // Gelesen-Status in der Liste
  items()[0].querySelector('.toggle').click();
  await until(() => items()[0].classList.contains('is-read'), 'als gelesen markiert');
  assert.equal(items()[0].querySelector('.toggle').textContent, 'Ungelesen');
  $('filter-read').value = 'false';
  $('filter-read').dispatchEvent(new window.Event('change'));
  await until(() => titles().length === 1, 'Filter ungelesen');
  assert.deepEqual(titles(), ['E2E Testartikel']);
  $('filter-read').value = 'true';
  $('filter-read').dispatchEvent(new window.Event('change'));
  await until(() => titles().length === 1 && titles()[0] === 'Kaffee und Bohnen', 'Filter gelesen');
  // Im Filter „gelesen“ verschwindet der Artikel, sobald er ungelesen wird.
  items()[0].querySelector('.toggle').click();
  await until(() => titles().length === 0, 'Artikel passt nicht mehr zum Filter');
  $('filter-read').value = '';
  $('filter-read').dispatchEvent(new window.Event('change'));
  await until(() => titles().length === 2, 'Filter zurückgesetzt');
  ok('Gelesen-Status umschalten und filtern');

  // Schlagwörter im Lesemodus
  window.location.hash = items()[1].querySelector('.article-main').getAttribute('href');
  await until(() => visible('view-reader') && !$('form-tags').hidden, 'Lesemodus mit Schlagwortfeld');
  assert.equal($('tags-input').value, '');
  assert.equal($('reader-toggle-read').textContent, 'Als gelesen markieren');
  $('tags-input').value = ' Rust , web dev,, rust ';
  $('form-tags').requestSubmit();
  await until(() => $('tags-input').value === 'Rust, web dev', 'Schlagwörter bereinigt zurück');
  $('reader-toggle-read').click();
  await until(() => $('reader-toggle-read').textContent === 'Als ungelesen markieren', 'im Lesemodus gelesen');
  $('tags-input').value = 'a,b';
  $('tags-input').value = 'x'.repeat(41);
  $('form-tags').requestSubmit();
  await until(() => visible('tags-error'), 'Fehlermeldung zu langes Schlagwort');
  assert.match($('tags-error').textContent, /höchstens 40/);
  ok('Schlagwörter und Gelesen-Status im Lesemodus');

  // Zurück: Leiste mit Schlagwörtern, Filter, Anzeige in der Liste
  window.location.hash = '#/';
  await until(() => visible('view-home') && !$('tag-bar').hidden, 'Schlagwortleiste');
  const chips = [...document.querySelectorAll('#tag-bar .tag-chip')];
  assert.deepEqual(chips.map((c) => c.firstChild.textContent), ['Rust', 'web dev']);
  assert.equal(chips[0].querySelector('.tag-count').textContent, '1');
  assert.match(items().find((li) => li.querySelector('.article-title').textContent === 'E2E Testartikel')
    .querySelector('.article-tags').textContent, /^#Rust #web dev$/);
  chips[0].click();
  await until(() => titles().length === 1, 'Filter nach Schlagwort');
  assert.deepEqual(titles(), ['E2E Testartikel']);
  assert.equal(document.querySelector('#tag-bar .tag-chip').getAttribute('aria-pressed'), 'true');
  document.querySelector('#tag-bar .tag-chip').click();
  await until(() => titles().length === 2, 'Schlagwortfilter aus');
  ok('Schlagwortleiste filtert und zeigt Schlagwörter in der Liste');

  // Letzten Artikel mit Schlagwörtern löschen: Leiste verschwindet
  items().find((li) => li.querySelector('.article-title').textContent === 'E2E Testartikel')
    .querySelector('.delete').click();
  await until(() => titles().length === 1 && $('tag-bar').hidden, 'Artikel gelöscht, Leiste weg');
  ok('Löschen räumt die Schlagwortleiste auf');
});
