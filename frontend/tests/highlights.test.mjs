// Frontend-Ablauf: Text markieren, Farbe wählen, Notiz anlegen, ein- und ausblenden, löschen.
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { ARTIKEL } from './fixtures.mjs';
import { setup, steps } from './helpers.mjs';

const { $, visible, until, site, teardown } = await setup({ '/artikel.html': ARTIKEL });
after(teardown);

/** Wählt Text wie der Benutzer (Auswahl setzen, danach feuert der Browser selectionchange). */
function selectText(startNode, startOffset, endNode, endOffset) {
  const range = document.createRange();
  range.setStart(startNode, startOffset);
  range.setEnd(endNode, endOffset);
  const selection = window.getSelection();
  selection.removeAllRanges();
  selection.addRange(range);
  document.dispatchEvent(new window.Event('selectionchange'));
}

const marks = () => [...$('reader-content').querySelectorAll('mark.hl')];
const color = (mark) => mark.style.getPropertyValue('--hl');
const swatches = () => [...$('hl-colors').querySelectorAll('.hl-swatch')];
const apiList = async (id) => (await fetch(`/api/articles/${id}/highlights`)).json();

test('Markieren, Farbe, Notiz, Ausblenden und Löschen im Lesemodus', async (t) => {
  const ok = steps(t);

  await until(() => visible('view-auth'), 'Anmeldemaske');
  $('tab-register').click();
  $('form-auth').elements.email.value = 'h@example.org';
  $('form-auth').elements.password.value = 'passwort-12345';
  $('form-auth').requestSubmit();
  await until(() => visible('view-home'), 'Startseite');

  $('add-url').value = site.url('/artikel.html');
  $('form-add').requestSubmit();
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'Artikel gespeichert');
  const id = (await (await fetch('/api/articles')).json())[0].id;

  const openArticle = async () => {
    window.location.hash = `#/article/${id}`;
    await until(() => visible('view-reader') && $('reader-content').children.length > 0, 'Lesemodus');
  };
  await openArticle();
  await until(() => $('hl-panel').hidden, 'Bedienfeld zu');
  assert.equal($('hl-toggle').hidden, true, 'ohne Markierungen kein Umschalter');
  assert.equal($('reader-notes').hidden, true);
  ok('Lesemodus ohne Markierungen');

  // ---- Auswahl innerhalb eines Absatzes -> Bedienfeld mit fünf Farben und eigener Farbe
  const paragraphs = [...$('reader-content').querySelectorAll('p')];
  const first = paragraphs[0].firstChild;
  const from = first.nodeValue.indexOf('ausführlicher');
  selectText(first, from, first, from + 'ausführlicher'.length);
  await until(() => !$('hl-panel').hidden, 'Bedienfeld nach Auswahl');
  assert.equal($('hl-title').textContent, 'Auswahl markieren');
  assert.equal($('hl-quote').textContent, 'ausführlicher');
  assert.equal(swatches().length, 5);
  assert.equal($('hl-colors').querySelector('input[type="color"]') !== null, true);
  assert.equal($('hl-edit').hidden, true, 'Notiz erst nach dem Markieren');
  ok('Auswahl öffnet das Bedienfeld mit Farben');

  swatches()[1].click(); // Grün
  await until(() => marks().length === 1, 'Markierung angelegt');
  assert.equal(marks()[0].textContent, 'ausführlicher');
  assert.equal(color(marks()[0]), '#6fcf97');
  assert.equal($('hl-title').textContent, 'Markierung');
  assert.equal(visible('hl-edit'), true, 'jetzt lässt sich eine Notiz anlegen');
  assert.equal(window.getSelection().isCollapsed, true);
  let saved = await apiList(id);
  assert.equal(saved.length, 1);
  assert.equal(saved[0].quote, 'ausführlicher');
  const at = $('reader-content').textContent.indexOf('ausführlicher');
  assert.equal(saved[0].start, at, 'Position im Gesamttext');
  assert.equal(saved[0].end, at + 'ausführlicher'.length);
  assert.equal(saved[0].note, null);
  assert.equal($('hl-toggle').hidden, false);
  ok('Farbe wählen legt die Markierung an');

  // ---- Notiz
  $('hl-note').value = 'Wichtig <b>nicht fett</b>\nzweite Zeile';
  $('hl-save').click();
  await until(() => $('reader-notes').querySelector('.note-text'), 'Notiz am Rand');
  const noteText = $('reader-notes').querySelector('.note-text');
  assert.equal(noteText.textContent, 'Wichtig <b>nicht fett</b>\nzweite Zeile');
  assert.equal(noteText.querySelector('b'), null, 'Notiz ist Text, kein HTML');
  assert.equal($('reader-notes').hidden, false);
  assert.equal(marks()[0].classList.contains('has-note'), true);
  assert.match($('reader-notes').textContent, /ausführlicher/);
  ok('Notiz erscheint neben dem Text');

  // ---- Farbe ändern: vorgegebene und eigene
  swatches()[3].click(); // Rosa
  await until(() => color(marks()[0]) === '#f78fb3', 'Farbe geändert');
  const custom = $('hl-colors').querySelector('input[type="color"]');
  custom.value = '#123456';
  custom.dispatchEvent(new window.Event('change', { bubbles: true }));
  await until(() => color(marks()[0]) === '#123456', 'eigene Farbe');
  assert.equal((await apiList(id))[0].color, '#123456');
  assert.equal((await apiList(id))[0].note, 'Wichtig <b>nicht fett</b>\nzweite Zeile', 'Notiz bleibt');
  assert.equal(swatches().length, 6, 'zuletzt gewählte eigene Farbe wird als Farbfeld angeboten');
  ok('Farbe ändern, auch eine eigene');

  // ---- Auswahl über mehrere Absätze
  const a = paragraphs[1].firstChild;
  const b = paragraphs[2].firstChild;
  selectText(a, 5, b, 20);
  await until(() => $('hl-title').textContent === 'Auswahl markieren', 'zweite Auswahl');
  swatches()[0].click();
  await until(() => marks().length >= 3, 'zweite Markierung (mehrere Teile)');
  saved = await apiList(id);
  assert.equal(saved.length, 2);
  const multi = saved.find((h) => h.quote.length > 100);
  assert.ok(multi, 'langer Wortlaut über Absatzgrenzen');
  const parts = marks().filter((m) => m.dataset.id === String(multi.id));
  assert.ok(parts.length >= 2, 'in mehrere Elemente aufgeteilt');
  assert.equal(parts.map((m) => m.textContent).join(''), multi.quote);
  assert.equal(parts.filter((m) => m.dataset.first === '1').length, 1);
  ok('Markierung über mehrere Absätze');

  // ---- Ausblenden und Einblenden
  $('hl-toggle').click();
  assert.equal($('reader-content').classList.contains('hl-off'), true);
  assert.equal($('reader-notes').hidden, true);
  assert.equal($('hl-toggle').textContent, 'Markierungen einblenden');
  assert.equal($('hl-panel').hidden, true);
  selectText(paragraphs[3].firstChild, 0, paragraphs[3].firstChild, 10);
  await new Promise((r) => setTimeout(r, 300));
  assert.equal($('hl-panel').hidden, true, 'bei ausgeblendeten Markierungen wird nichts angelegt');
  window.getSelection().removeAllRanges();
  $('hl-toggle').click();
  assert.equal($('reader-content').classList.contains('hl-off'), false);
  assert.equal($('reader-notes').hidden, false);
  ok('Markierungen ausblenden und einblenden');

  // ---- Nach dem Neuladen sind Markierungen und Notiz wieder da
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  assert.equal($('hl-panel').hidden, true);
  await openArticle();
  await until(() => marks().length >= 3, 'Markierungen nach dem Neuladen');
  assert.equal(marks()[0].textContent, 'ausführlicher');
  assert.equal(color(marks()[0]), '#123456');
  assert.match($('reader-notes').textContent, /zweite Zeile/);
  assert.equal($('reader-content').textContent.includes('undefined'), false);
  ok('Markierungen bleiben erhalten');

  // ---- Notiz am Rand öffnet die Markierung zum Bearbeiten
  $('reader-notes').querySelector('.note').click();
  await until(() => !$('hl-panel').hidden && $('hl-title').textContent === 'Markierung', 'Bearbeiten über Notiz');
  assert.equal($('hl-note').value, 'Wichtig <b>nicht fett</b>\nzweite Zeile');
  $('hl-note').value = '';
  $('hl-save').click();
  await until(() => $('reader-notes').querySelector('.note') === null, 'Notiz entfernt');
  assert.equal((await apiList(id))[0].note, null);
  assert.equal(marks()[0].classList.contains('has-note'), false);
  ok('Notiz entfernen');

  // ---- Löschen über Klick auf die Markierung
  marks()[0].click();
  await until(() => !$('hl-panel').hidden && visible('hl-edit'), 'Bedienfeld per Klick');
  assert.equal($('hl-quote').textContent, 'ausführlicher');
  $('hl-delete').click();
  await until(() => !marks().some((m) => m.textContent === 'ausführlicher'), 'Markierung gelöscht');
  assert.equal($('hl-panel').hidden, true);
  assert.equal((await apiList(id)).length, 1);
  assert.equal($('reader-content').textContent.includes('ausführlicher'), true, 'der Text bleibt');
  ok('Markierung löschen');

  // ---- Zu lange Auswahl wird abgelehnt, ohne etwas anzulegen
  const long = $('reader-content');
  const walker = document.createTreeWalker(long, 4);
  const firstNode = walker.nextNode();
  let lastNode = firstNode;
  for (let n = walker.nextNode(); n; n = walker.nextNode()) lastNode = n;
  const body = long.textContent.length;
  // Der Testartikel ist kürzer als 2000 Zeichen: der Text wird dafür künstlich verlängert.
  lastNode.nodeValue += 'x'.repeat(2500);
  selectText(firstNode, 0, lastNode, lastNode.nodeValue.length);
  await until(() => $('hl-quote').textContent === '' && !$('hl-error').hidden, 'Fehler bei langer Auswahl');
  assert.match($('hl-error').textContent, /zu lang/);
  assert.equal(swatches().every((s) => s.disabled), true);
  assert.equal((await apiList(id)).length, 1);
  assert.ok(body > 0);
  ok('zu lange Auswahl wird abgelehnt');
});
