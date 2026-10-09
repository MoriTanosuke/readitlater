// Frontend-Ablauf: Registrierung, Artikel speichern/lesen/löschen, Abmelden (echtes Backend, jsdom).
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { ARTIKEL, KAFFEE } from './fixtures.mjs';
import { setup, steps } from './helpers.mjs';

const { $, visible, until, site, teardown } = await setup({
  '/artikel.html': ARTIKEL,
  '/zweiter.html': KAFFEE,
});
after(teardown);
const SITE = site.url('/artikel.html');

test('Anmelden, Artikel speichern, lesen und löschen', async (t) => {
  const ok = steps(t);
  await until(() => visible('view-auth'), 'Anmeldemaske');
  ok('ohne Sitzung wird die Anmeldung gezeigt');

  // Registrieren
  $('tab-register').click();
  const form = $('form-auth');
  form.elements.email.value = 'e2e@example.org';
  form.elements.password.value = 'passwort-12345';
  form.requestSubmit();
  await until(() => visible('view-home'), 'Startseite nach Registrierung');
  assert.equal($('user-email').textContent, 'e2e@example.org');
  await until(() => visible('list-empty'), 'Hinweis „noch keine Artikel“');
  ok('Registrierung meldet an, leere Liste mit Hinweis');

  // Artikel speichern
  $('add-url').value = SITE;
  $('form-add').requestSubmit();
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'Artikel in der Liste');
  assert.equal($('add-url').value, '', 'Eingabefeld wird geleert');
  assert.equal($('list-empty').hidden, true);
  const item = document.querySelector('#article-list li');
  assert.equal(item.querySelector('.article-title').textContent, 'E2E Testartikel');
  assert.match(item.querySelector('.article-meta').textContent, /^127\.0\.0\.1 · \d\d\.\d\d\.\d{4}$/);
  assert.ok(item.querySelector('.article-excerpt').textContent.length > 20);
  ok('Artikel wird gespeichert und in der Liste angezeigt');

  // Doppelt speichern → Fehlermeldung, Liste unverändert
  $('add-url').value = `${SITE}#anker`;
  $('form-add').requestSubmit();
  await until(() => visible('add-error'), 'Fehlermeldung bei Duplikat');
  assert.match($('add-error').textContent, /bereits gespeichert/);
  assert.equal(document.querySelectorAll('#article-list li').length, 1);
  ok('doppelte Adresse zeigt Fehlermeldung');

  // Ungültige Adresse
  $('add-url').value = 'ftp://example.com/x';
  $('form-add').requestSubmit();
  await until(() => /Ungültige Adresse/.test($('add-error').textContent), 'Fehlermeldung bei ungültiger Adresse');
  ok('ungültige Adresse zeigt Fehlermeldung');

  // Lesemodus
  const href = item.querySelector('.article-main').getAttribute('href');
  assert.match(href, /^#\/article\/\d+$/);
  window.location.hash = href;
  await until(() => visible('view-reader') && $('reader-content').children.length > 0, 'Lesemodus');
  assert.equal($('reader-title').textContent, 'E2E Testartikel');
  assert.match($('reader-meta').textContent, /127\.0\.0\.1 · gespeichert am/);
  const content = $('reader-content');
  assert.match(content.textContent, /ausführlicher Absatz/);
  assert.equal(content.querySelector('script'), null);
  assert.equal(content.querySelector('[onclick], [onerror]'), null);
  for (const a of content.querySelectorAll('a')) assert.notEqual(a.getAttribute('href')?.startsWith('javascript:'), true);
  assert.equal(window.pwned, undefined);
  assert.equal(document.title, 'E2E Testartikel – Leseliste');
  ok('Lesemodus zeigt bereinigten Text ohne Skripte');

  // Zurück zur Liste
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'zurück zur Liste');
  assert.equal(document.querySelectorAll('#article-list li').length, 1);
  assert.equal(document.title, 'Leseliste');
  ok('Rückkehr zur Liste');

  // Unbekannter Artikel
  window.location.hash = '#/article/99999';
  await until(() => visible('reader-error'), 'Fehler für unbekannten Artikel');
  assert.match($('reader-error').textContent, /nicht gefunden/);
  ok('unbekannter Artikel zeigt Fehlermeldung');
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'zurück');

  // Löschen aus dem Lesemodus
  window.location.hash = href;
  await until(() => visible('view-reader') && !$('reader-delete').hidden, 'Lesemodus (Löschen)');
  $('reader-delete').click();
  await until(() => visible('view-home'), 'Liste nach Löschen');
  await until(() => document.querySelectorAll('#article-list li').length === 0 && visible('list-empty'), 'leere Liste');
  ok('Löschen im Lesemodus entfernt den Artikel');

  // Erneut speichern und aus der Liste löschen
  $('add-url').value = SITE;
  $('form-add').requestSubmit();
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'Artikel erneut gespeichert');
  document.querySelector('#article-list .delete').click();
  await until(() => document.querySelectorAll('#article-list li').length === 0, 'Artikel aus Liste gelöscht');
  ok('Löschen aus der Liste funktioniert');

  // Abmelden und wieder anmelden
  $('btn-logout').click();
  await until(() => visible('view-auth'), 'Anmeldung nach Logout');
  $('tab-login').click();
  form.elements.email.value = 'e2e@example.org';
  form.elements.password.value = 'falsches-passwort';
  form.requestSubmit();
  await until(() => visible('auth-error'), 'Fehler bei falschem Passwort');
  assert.match($('auth-error').textContent, /falsch/);
  form.elements.password.value = 'passwort-12345';
  form.requestSubmit();
  await until(() => visible('view-home'), 'Startseite nach Login');
  ok('Logout, falsches und richtiges Passwort beim Login');
});
